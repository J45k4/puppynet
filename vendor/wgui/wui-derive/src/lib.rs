use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
	parse::{Parse, ParseStream},
	parse_macro_input, Data, DeriveInput, Fields, FnArg, ImplItem, ItemImpl, LitStr, ReturnType,
	Token, Type,
};

#[proc_macro_derive(WguiModel)]
pub fn derive_wgui_model(input: TokenStream) -> TokenStream {
	derive_wui_value_convert(input, "WguiModel")
}

#[proc_macro_derive(WuiModel)]
pub fn derive_wui_model(input: TokenStream) -> TokenStream {
	derive_wui_value_convert(input, "WuiModel")
}

#[proc_macro_derive(Wdb)]
pub fn derive_wdb(input: TokenStream) -> TokenStream {
	let input = parse_macro_input!(input as DeriveInput);
	let name = input.ident;
	let fields = match input.data {
		Data::Struct(data) => data.fields,
		_ => {
			return syn::Error::new_spanned(name, "Wdb can only be derived for structs")
				.to_compile_error()
				.into();
		}
	};

	let named = match fields {
		Fields::Named(named) => named.named,
		_ => {
			return syn::Error::new_spanned(name, "Wdb requires named fields")
				.to_compile_error()
				.into();
		}
	};

	let model_schemas = named.iter().map(|field| {
		let ty = &field.ty;
		quote! { <#ty as wgui::wui::runtime::WdbModel>::schema() }
	});

	let expanded = quote! {
		impl wgui::wui::runtime::WdbSchema for #name {
			fn schema() -> ::std::vec::Vec<wgui::wui::runtime::WdbModelSchema> {
				vec![#(#model_schemas),*]
			}
		}
	};

	expanded.into()
}

fn derive_wui_value_convert(input: TokenStream, label: &str) -> TokenStream {
	let input = parse_macro_input!(input as DeriveInput);
	let name = input.ident;
	let fields = match input.data {
		Data::Struct(data) => data.fields,
		_ => {
			return syn::Error::new_spanned(
				name,
				format!("{label} can only be derived for structs"),
			)
			.to_compile_error()
			.into();
		}
	};

	let named = match fields {
		Fields::Named(named) => named.named,
		_ => {
			return syn::Error::new_spanned(name, format!("{label} requires named fields"))
				.to_compile_error()
				.into();
		}
	};

	let entries = named.iter().map(|field| {
		let ident = field.ident.as_ref().unwrap();
		let key = ident.to_string();
		quote! {
			(#key.to_string(), wgui::wui::runtime::WuiValueConvert::to_wui_value(&self.#ident))
		}
	});
	let schema_fields = named.iter().map(|field| {
		let ident = field.ident.as_ref().unwrap();
		let key = ident.to_string();
		let ty = &field.ty;
		quote! {
			wgui::wui::runtime::WdbFieldSchema {
				name: #key,
				rust_type: stringify!(#ty),
			}
		}
	});

	let expanded = quote! {
		impl wgui::wui::runtime::WuiValueConvert for #name {
			fn to_wui_value(&self) -> wgui::wui::runtime::WuiValue {
				wgui::wui::runtime::WuiValue::object(vec![
					#(#entries),*
				])
			}
		}

		impl wgui::wui::runtime::WdbModel for #name {
			fn schema() -> wgui::wui::runtime::WdbModelSchema {
				wgui::wui::runtime::WdbModelSchema {
					model: stringify!(#name),
					fields: vec![#(#schema_fields),*],
				}
			}
		}
	};

	expanded.into()
}

#[proc_macro_attribute]
pub fn wgui_controller(attr: TokenStream, item: TokenStream) -> TokenStream {
	let args = parse_macro_input!(attr as WguiControllerArgs);
	let impl_block = parse_macro_input!(item as ItemImpl);
	match expand_wgui_controller(args, impl_block) {
		Ok(tokens) => tokens,
		Err(err) => err.to_compile_error().into(),
	}
}

#[derive(Default)]
struct WguiControllerArgs {
	template: Option<String>,
}

impl Parse for WguiControllerArgs {
	fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
		let mut args = Self::default();
		while !input.is_empty() {
			let ident: syn::Ident = input.parse()?;
			input.parse::<Token![=]>()?;
			if ident == "template" {
				let value: LitStr = input.parse()?;
				args.template = Some(value.value());
			} else {
				return Err(syn::Error::new_spanned(
					ident,
					"unsupported wgui_controller argument",
				));
			}
			if input.is_empty() {
				break;
			}
			input.parse::<Token![,]>()?;
		}
		Ok(args)
	}
}

enum HandlerArg {
	None,
	U32,
	I32,
	U32I32,
	String,
	Json,
}

struct HandlerMethod {
	ident: syn::Ident,
	arg: HandlerArg,
	is_async: bool,
}

struct FallbackEventHandler {
	ident: syn::Ident,
	is_async: bool,
}

enum TitleReturn {
	String,
	OptionString,
}

fn expand_wgui_controller(
	args: WguiControllerArgs,
	impl_block: ItemImpl,
) -> syn::Result<TokenStream> {
	let controller_ident = match *impl_block.self_ty.clone() {
		Type::Path(path) => path
			.path
			.segments
			.last()
			.map(|seg| seg.ident.clone())
			.ok_or_else(|| syn::Error::new_spanned(path, "wgui_controller requires a type name"))?,
		other => {
			return Err(syn::Error::new_spanned(
				other,
				"wgui_controller only supports impl blocks for named types",
			))
		}
	};

	let mut state_method: Option<(syn::Ident, Type)> = None;
	let mut fallback_model_methods: Vec<(syn::Ident, Type)> = Vec::new();
	let mut title_method: Option<(syn::Ident, TitleReturn)> = None;
	let mut handlers = Vec::new();
	let mut fallback_event_handler: Option<FallbackEventHandler> = None;

	for item in &impl_block.items {
		let ImplItem::Fn(method) = item else {
			continue;
		};

		let receiver = method.sig.inputs.first();
		let Some(FnArg::Receiver(recv)) = receiver else {
			continue;
		};

		let input_count = method
			.sig
			.inputs
			.iter()
			.filter(|arg| !matches!(arg, FnArg::Receiver(_)))
			.count();

		match (&recv.reference, &recv.mutability) {
			(Some(_), None) => {
				if input_count == 0 {
					if method.sig.asyncness.is_some() {
						continue;
					}
					if let ReturnType::Type(_, ty) = &method.sig.output {
						if matches!(**ty, Type::Tuple(_)) {
							continue;
						}
						if method.sig.ident == "title" {
							if title_method.is_some() {
								return Err(syn::Error::new_spanned(
									&method.sig.ident,
									"wgui_controller allows only one title method",
								));
							}
							title_method =
								Some((method.sig.ident.clone(), title_return_from_type(ty)?));
						} else if method.sig.ident == "state" {
							if state_method.is_some() {
								return Err(syn::Error::new_spanned(
									&method.sig.ident,
									"wgui_controller allows only one state method",
								));
							}
							state_method = Some((method.sig.ident.clone(), (**ty).clone()));
						} else {
							fallback_model_methods.push((method.sig.ident.clone(), (**ty).clone()));
						}
					}
				}
			}
			(Some(_), Some(_)) => {
				let arg_type = method.sig.inputs.iter().find_map(|arg| match arg {
					FnArg::Typed(pat) => Some(&*pat.ty),
					_ => None,
				});
				if input_count == 1 {
					if let Some(arg_type) = arg_type {
						if is_client_event_ref(arg_type) {
							if fallback_event_handler.is_some() {
								return Err(syn::Error::new_spanned(
									&method.sig.ident,
									"wgui_controller allows only one &mut self method that accepts &ClientEvent",
								));
							}
							fallback_event_handler = Some(FallbackEventHandler {
								ident: method.sig.ident.clone(),
								is_async: method.sig.asyncness.is_some(),
							});
							continue;
						}
					}
				}

				let arg = match input_count {
					0 => Some(HandlerArg::None),
					1 => arg_type.and_then(handler_arg_from_type),
					2 => handler_two_args_from_method(method),
					_ => None,
				};
				if let Some(arg) = arg {
					handlers.push(HandlerMethod {
						ident: method.sig.ident.clone(),
						arg,
						is_async: method.sig.asyncness.is_some(),
					});
				}
			}
			_ => {}
		}
	}

	let (model_method_ident, model_type) = if let Some(state_method) = state_method {
		state_method
	} else {
		if fallback_model_methods.len() > 1 {
			return Err(syn::Error::new_spanned(
				&controller_ident,
				"wgui_controller requires exactly one &self method returning a model, or a method named state",
			));
		}
		fallback_model_methods.pop().ok_or_else(|| {
			syn::Error::new_spanned(
				&controller_ident,
				"wgui_controller requires an &self method that returns a model",
			)
		})?
	};

	let model_type_ident = match &model_type {
		Type::Path(path) => path
			.path
			.segments
			.last()
			.map(|seg| seg.ident.to_string())
			.ok_or_else(|| {
				syn::Error::new_spanned(model_type.clone(), "model type must be a named type")
			})?,
		_ => {
			return Err(syn::Error::new_spanned(
				model_type.clone(),
				"model type must be a named type",
			))
		}
	};
	let direct_item_render = model_method_ident == "render" && model_type_ident == "Item";

	let module_name = {
		let mut name = to_snake_case(&model_type_ident);
		if name.ends_with("_state") {
			name.truncate(name.len() - "_state".len());
		}
		name
	};

	let explicit_template = if let Some(template) = args.template {
		quote! { ::std::option::Option::Some(#template.to_string()) }
	} else {
		quote! { ::std::option::Option::None }
	};
	let template_fn = format_ident!("__wgui_template_for_{}", controller_ident);
	let action_fn = format_ident!("__wgui_action_name_for_{}", controller_ident);
	let module_name_fn = format_ident!("__wgui_module_name_for_{}", controller_ident);
	let title_impl = title_method.map(|(ident, return_type)| match return_type {
		TitleReturn::String => quote! {
			fn title(&self) -> ::std::option::Option<::std::string::String> {
				::std::option::Option::Some(self.#ident())
			}
		},
		TitleReturn::OptionString => quote! {
			fn title(&self) -> ::std::option::Option<::std::string::String> {
				self.#ident()
			}
		},
	});

	let no_arg_arms = handlers
		.iter()
		.filter(|handler| matches!(handler.arg, HandlerArg::None))
		.map(|handler| {
			let ident = &handler.ident;
			let name = ident.to_string();
			if handler.is_async {
				quote! { #name => { self.#ident().await; true } }
			} else {
				quote! { #name => { self.#ident(); true } }
			}
		});
	let u32_arms = handlers
		.iter()
		.filter(|handler| matches!(handler.arg, HandlerArg::U32))
		.map(|handler| {
			let ident = &handler.ident;
			let name = ident.to_string();
			if handler.is_async {
				quote! { #name => { self.#ident(arg).await; true } }
			} else {
				quote! { #name => { self.#ident(arg); true } }
			}
		});
	let i32_arms = handlers
		.iter()
		.filter(|handler| matches!(handler.arg, HandlerArg::I32))
		.map(|handler| {
			let ident = &handler.ident;
			let name = ident.to_string();
			if handler.is_async {
				quote! { #name => { self.#ident(value).await; true } }
			} else {
				quote! { #name => { self.#ident(value); true } }
			}
		});
	let u32_i32_arms = handlers
		.iter()
		.filter(|handler| matches!(handler.arg, HandlerArg::U32I32))
		.map(|handler| {
			let ident = &handler.ident;
			let name = ident.to_string();
			if handler.is_async {
				quote! { #name => { self.#ident(arg, value).await; true } }
			} else {
				quote! { #name => { self.#ident(arg, value); true } }
			}
		});
	let string_arms = handlers
		.iter()
		.filter(|handler| matches!(handler.arg, HandlerArg::String))
		.map(|handler| {
			let ident = &handler.ident;
			let name = ident.to_string();
			if handler.is_async {
				quote! { #name => { self.#ident(value).await; true } }
			} else {
				quote! { #name => { self.#ident(value); true } }
			}
		})
		.collect::<Vec<_>>();
	let string_arms_ref = &string_arms;
	let json_arms = handlers
		.iter()
		.filter(|handler| matches!(handler.arg, HandlerArg::Json))
		.map(|handler| {
			let ident = &handler.ident;
			let name = ident.to_string();
			if handler.is_async {
				quote! { #name => { self.#ident(payload).await; true } }
			} else {
				quote! { #name => { self.#ident(payload); true } }
			}
		});
	let fallback_decode = if let Some(handler) = &fallback_event_handler {
		let ident = &handler.ident;
		if handler.is_async {
			quote! {
				return self.#ident(event).await;
			}
		} else {
			quote! {
				return self.#ident(event);
			}
		}
	} else {
		quote! {
			return false;
		}
	};

	if direct_item_render {
		let output = quote! {
			#impl_block

			#[::wgui::wui::runtime::async_trait]
			impl ::wgui::wui::runtime::WuiController for #controller_ident {
				fn render(&self) -> ::wgui::Item {
					self.#model_method_ident()
				}

				#title_impl

				async fn handle(&mut self, event: &::wgui::ClientEvent) -> bool {
					#fallback_decode
				}
			}
		};

		return Ok(output.into());
	}

	let output = quote! {
		#impl_block

		#[allow(non_snake_case)]
		fn #module_name_fn() -> ::std::vec::Vec<::std::string::String> {
			if let ::std::option::Option::Some(explicit) = #explicit_template {
				return ::std::vec![explicit];
			}

			let fallback = #module_name;
			let path = ::std::path::Path::new(file!());
			let stem = path
				.file_stem()
				.and_then(|value| value.to_str())
				.unwrap_or("");

			let old_derived = if stem == "mod" {
				path.parent()
					.and_then(|parent| parent.file_name())
					.and_then(|value| value.to_str())
					.unwrap_or("")
			} else {
				stem
			};

			let mut candidates = ::std::vec::Vec::new();
			let parts = path
				.components()
				.filter_map(|component| match component {
					::std::path::Component::Normal(value) => value.to_str(),
					_ => ::std::option::Option::None,
				})
				.collect::<::std::vec::Vec<_>>();
			if let ::std::option::Option::Some(src_index) = parts.iter().rposition(|part| *part == "src") {
				let mut module_parts = parts
					.iter()
					.skip(src_index + 1)
					.map(|part| (*part).to_string())
					.collect::<::std::vec::Vec<_>>();
				if let ::std::option::Option::Some(last) = module_parts.last_mut() {
					if let ::std::option::Option::Some(stripped) = last.strip_suffix(".rs") {
						*last = stripped.to_string();
					}
				}
				if module_parts.last().map(|part| part == "mod").unwrap_or(false) {
					module_parts.pop();
				}
				if !module_parts.is_empty() {
					candidates.push(module_parts.join("/"));
				}
			}
			if !old_derived.is_empty() {
				candidates.push(old_derived.to_string());
			}
			candidates.push(fallback.to_string());

			let mut unique = ::std::vec::Vec::new();
			for candidate in candidates {
				if !unique.iter().any(|existing| existing == &candidate) {
					unique.push(candidate);
				}
			}
			unique
		}

	#[allow(non_snake_case)]
	fn #template_fn() -> &'static ::wgui::wui::runtime::Template {
		static TEMPLATE: ::std::sync::OnceLock<::wgui::wui::runtime::Template> = ::std::sync::OnceLock::new();
		TEMPLATE.get_or_init(|| {
			let base_dir = ::std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/wui"));
			let candidates = #module_name_fn();
			let mut read_errors = ::std::vec::Vec::new();
			for module_name in candidates {
				let source_path = base_dir.join(format!("{}.wui", module_name));
				let source = match ::std::fs::read_to_string(&source_path) {
					::std::result::Result::Ok(source) => source,
					::std::result::Result::Err(err) => {
						read_errors.push(format!("{}: {}", source_path.display(), err));
						continue;
					}
				};
				return ::wgui::wui::runtime::Template::parse_with_dir(&source, &module_name, source_path.parent())
					.unwrap_or_else(|diags| panic!("failed to parse wui template {}: {:?}", module_name, diags));
			}
			panic!("failed to read wui template; tried {}", read_errors.join(", "))
		})
	}

	#[allow(non_snake_case)]
	fn #action_fn(name: &str) -> ::std::string::String {
			let mut out = ::std::string::String::with_capacity(name.len());
			for (index, ch) in name.chars().enumerate() {
				if ch.is_uppercase() {
					if index != 0 {
						out.push('_');
					}
					for lower in ch.to_lowercase() {
						out.push(lower);
					}
				} else {
					out.push(ch);
				}
			}
			out
		}

	#[::wgui::wui::runtime::async_trait]
	impl ::wgui::wui::runtime::WuiController for #controller_ident {
		fn render(&self) -> ::wgui::Item {
			let model = self.#model_method_ident();
			#template_fn().render(&model)
		}

		fn render_with_path(&self, path: &str) -> ::wgui::Item {
			let model = self.#model_method_ident();
			#template_fn().render_with_path(&model, path)
		}

		fn render_with_route(
			&self,
			route: &::wgui::wui::runtime::RouteContext,
		) -> ::wgui::Item {
			let model = self.#model_method_ident();
			#template_fn().render_with_route(&model, route)
		}

		#title_impl

		fn route_title(&self, path: &str) -> ::std::option::Option<::std::string::String> {
			#template_fn().title_for_path(path)
		}

		async fn handle(&mut self, event: &::wgui::ClientEvent) -> bool {
			let Some(action) = #template_fn().decode(event) else {
				#fallback_decode
			};
				match action {
					::wgui::wui::runtime::RuntimeAction::Click { ref name, arg } => {
						let action_name = #action_fn(name);
						if let Some(arg) = arg {
							match action_name.as_str() {
								#(#u32_arms,)*
								_ => false,
							}
						} else {
							match action_name.as_str() {
								#(#no_arg_arms,)*
								_ => false,
							}
						}
					}
					::wgui::wui::runtime::RuntimeAction::TextChanged { ref name, value } => {
						let action_name = #action_fn(name);
						match action_name.as_str() {
							#(#string_arms_ref,)*
							_ => false,
						}
					}
					::wgui::wui::runtime::RuntimeAction::SliderChange { ref name, arg, value } => {
						let action_name = #action_fn(name);
						if let Some(arg) = arg {
							match action_name.as_str() {
								#(#u32_i32_arms,)*
								_ => false,
							}
						} else {
							match action_name.as_str() {
								#(#i32_arms,)*
								_ => false,
							}
						}
					}
					::wgui::wui::runtime::RuntimeAction::Select { ref name, value } => {
						let action_name = #action_fn(name);
						match action_name.as_str() {
							#(#string_arms_ref,)*
							_ => false,
						}
					}
					::wgui::wui::runtime::RuntimeAction::Custom { ref name, payload, .. } => {
						let action_name = #action_fn(name);
						match action_name.as_str() {
							#(#json_arms,)*
							_ => false,
						}
					}
				}
			}
		}
	};

	Ok(output.into())
}

fn handler_arg_from_type(ty: &Type) -> Option<HandlerArg> {
	let Type::Path(path) = ty else {
		return None;
	};
	let ident = path.path.segments.last()?.ident.to_string();
	match ident.as_str() {
		"String" => Some(HandlerArg::String),
		"u32" => Some(HandlerArg::U32),
		"i32" => Some(HandlerArg::I32),
		"Value" => Some(HandlerArg::Json),
		_ => None,
	}
}

fn handler_two_args_from_method(method: &syn::ImplItemFn) -> Option<HandlerArg> {
	let mut types = method.sig.inputs.iter().filter_map(|arg| match arg {
		FnArg::Typed(pat) => Some(&*pat.ty),
		_ => None,
	});
	let first = types.next()?;
	let second = types.next()?;
	if matches!(handler_arg_from_type(first), Some(HandlerArg::U32))
		&& matches!(handler_arg_from_type(second), Some(HandlerArg::I32))
	{
		Some(HandlerArg::U32I32)
	} else {
		None
	}
}

fn title_return_from_type(ty: &Type) -> syn::Result<TitleReturn> {
	let Type::Path(path) = ty else {
		return Err(syn::Error::new_spanned(
			ty,
			"title must return String or Option<String>",
		));
	};
	let Some(segment) = path.path.segments.last() else {
		return Err(syn::Error::new_spanned(
			ty,
			"title must return String or Option<String>",
		));
	};
	if segment.ident == "String" {
		return Ok(TitleReturn::String);
	}
	if segment.ident != "Option" {
		return Err(syn::Error::new_spanned(
			ty,
			"title must return String or Option<String>",
		));
	}
	let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
		return Err(syn::Error::new_spanned(
			ty,
			"title must return String or Option<String>",
		));
	};
	let Some(syn::GenericArgument::Type(Type::Path(inner))) = args.args.first() else {
		return Err(syn::Error::new_spanned(
			ty,
			"title must return String or Option<String>",
		));
	};
	let is_string = inner
		.path
		.segments
		.last()
		.map(|segment| segment.ident == "String")
		.unwrap_or(false);
	if is_string {
		Ok(TitleReturn::OptionString)
	} else {
		Err(syn::Error::new_spanned(
			ty,
			"title must return String or Option<String>",
		))
	}
}

fn is_client_event_ref(ty: &Type) -> bool {
	let Type::Reference(reference) = ty else {
		return false;
	};
	let Type::Path(path) = reference.elem.as_ref() else {
		return false;
	};
	path.path
		.segments
		.last()
		.map(|segment| segment.ident == "ClientEvent")
		.unwrap_or(false)
}

fn to_snake_case(value: &str) -> String {
	let mut out = String::with_capacity(value.len());
	for (index, ch) in value.chars().enumerate() {
		if ch.is_uppercase() {
			if index != 0 {
				out.push('_');
			}
			for lower in ch.to_lowercase() {
				out.push(lower);
			}
		} else {
			out.push(ch);
		}
	}
	out
}
