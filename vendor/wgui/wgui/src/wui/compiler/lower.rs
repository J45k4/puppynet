use crate::wui::ast::{AttrValue, Element, Expr, Node};
use crate::wui::compiler::ir::{
	ActionDef, ActionPayload, EventKind, IrComponent, IrDocument, IrFor, IrIf, IrNode, IrProp,
	IrRoute, IrScope, IrSwitch, IrWidget, PageMeta,
};
use crate::wui::compiler::registry::schema_for;
use crate::wui::diagnostic::{Diagnostic, Span};
use std::collections::HashMap;

pub fn lower(
	doc: &crate::wui::compiler::validate::ValidatedDocument,
	module_name: &str,
	diags: &mut Vec<Diagnostic>,
) -> IrDocument {
	let mut ctx = LowerContext::new(module_name);
	let nodes = lower_nodes(&doc.nodes, &mut ctx, diags);
	let mut components = HashMap::new();
	for (name, nodes) in &doc.components {
		let body = lower_nodes(nodes, &mut ctx, diags);
		components.insert(name.clone(), IrComponent { body });
	}
	IrDocument {
		nodes,
		components,
		actions: ctx.actions,
		pages: ctx.pages,
	}
}

struct LowerContext {
	module: String,
	scope_stack: Vec<String>,
	actions: Vec<ActionDef>,
	action_ids: HashMap<u32, String>,
	pages: Vec<PageMeta>,
}

impl LowerContext {
	fn new(module_name: &str) -> Self {
		Self {
			module: module_name.to_string(),
			scope_stack: Vec::new(),
			actions: Vec::new(),
			action_ids: HashMap::new(),
			pages: Vec::new(),
		}
	}

	fn scoped_action(&self, name: &str) -> String {
		if self.scope_stack.is_empty() {
			name.to_string()
		} else {
			format!("{}/{}", self.scope_stack.join("/"), name)
		}
	}

	fn add_action(
		&mut self,
		name: String,
		kind: EventKind,
		payload: ActionPayload,
		span: Span,
		diags: &mut Vec<Diagnostic>,
	) {
		let id = hash_action_id(&name);
		if let Some(existing) = self.action_ids.get(&id) {
			if existing != &name {
				diags.push(Diagnostic::new(
					format!("action id collision between {} and {}", existing, name),
					span,
				));
			}
		} else {
			self.action_ids.insert(id, name.clone());
		}
		if let Some(existing) = self.actions.iter().find(|a| a.name == name) {
			if existing.kind != kind || existing.payload != payload {
				diags.push(Diagnostic::new(
					format!("action {} used with conflicting payloads", name),
					span,
				));
			}
			return;
		}
		self.actions.push(ActionDef {
			name,
			kind,
			payload,
			id,
		});
	}
}

fn lower_nodes(nodes: &[Node], ctx: &mut LowerContext, diags: &mut Vec<Diagnostic>) -> Vec<IrNode> {
	let mut out = Vec::new();
	let mut i = 0;
	while i < nodes.len() {
		match &nodes[i] {
			Node::Element(el) if el.name == "If" => {
				let then_body = lower_nodes(&el.children, ctx, diags);
				let mut else_body = Vec::new();
				if i + 1 < nodes.len() {
					if let Node::Element(next) = &nodes[i + 1] {
						if next.name == "Else" {
							else_body = lower_nodes(&next.children, ctx, diags);
							i += 1;
						}
					}
				}
				if let Some(test) = get_expr_attr(el, "test") {
					out.push(IrNode::If(IrIf {
						test,
						then_body,
						else_body,
					}));
				}
			}
			Node::Element(el) if el.name == "For" => {
				let each = get_expr_attr(el, "each")
					.unwrap_or_else(|| Expr::Literal(crate::wui::ast::Literal::Null, el.span));
				let item = get_string_attr(el, "itemAs").unwrap_or_else(|| "item".to_string());
				let index = get_string_attr(el, "indexAs");
				let key = get_expr_attr(el, "key");
				let body = lower_nodes(&el.children, ctx, diags);
				out.push(IrNode::For(IrFor {
					each,
					item,
					index,
					key,
					body,
				}));
			}
			Node::Element(el) if el.name == "Scope" => {
				let name = get_string_attr(el, "name").unwrap_or_else(|| "scope".to_string());
				ctx.scope_stack.push(name.clone());
				let body = lower_nodes(&el.children, ctx, diags);
				ctx.scope_stack.pop();
				out.push(IrNode::Scope(IrScope { name, body }));
			}
			Node::Element(el) if el.name == "Page" => {
				diags.push(Diagnostic::new(
					"Page is deprecated; use Route instead",
					el.span,
				));
				let route = get_string_attr(el, "route");
				let title = get_string_attr(el, "title");
				let state_type = get_string_attr(el, "state");
				ctx.pages.push(PageMeta {
					module: ctx.module.clone(),
					route,
					title,
					state_type,
				});
			}
			Node::Element(el) if el.name == "Route" => {
				let route = get_string_attr(el, "path").or_else(|| get_string_attr(el, "route"));
				let title = get_string_attr(el, "title");
				let state_type = get_string_attr(el, "state");
				ctx.pages.push(PageMeta {
					module: ctx.module.clone(),
					route: route.clone(),
					title,
					state_type,
				});
				if let Some(path) = route {
					let body = lower_nodes(&el.children, ctx, diags);
					out.push(IrNode::Route(IrRoute { path, body }));
				}
			}
			Node::Element(el) if el.name == "Switch" => {
				let mut cases = Vec::new();
				for child in &el.children {
					let Node::Element(case_el) = child else {
						continue;
					};
					if case_el.name != "Case" {
						continue;
					}
					let route = get_string_attr(case_el, "path")
						.or_else(|| get_string_attr(case_el, "route"));
					let title = get_string_attr(case_el, "title");
					let state_type = get_string_attr(case_el, "state");
					ctx.pages.push(PageMeta {
						module: ctx.module.clone(),
						route: route.clone(),
						title,
						state_type,
					});
					if let Some(path) = route {
						let body = lower_nodes(&case_el.children, ctx, diags);
						cases.push(IrRoute { path, body });
					}
				}
				out.push(IrNode::Switch(IrSwitch { cases }));
			}
			Node::Element(el) if el.name == "Children" => {
				out.push(IrNode::Children);
			}
			Node::Element(el) => {
				if let Some(widget) = lower_widget(el, ctx, diags) {
					out.push(IrNode::Widget(widget));
				}
			}
			Node::Text(text, _) => out.push(IrNode::Text(text.clone())),
			Node::Expr(_) => {
				diags.push(Diagnostic::new(
					"bare expressions are not supported as nodes",
					el_span(nodes[i].clone()),
				));
			}
		}
		i += 1;
	}
	out
}

fn el_span(node: Node) -> Span {
	match node {
		Node::Element(el) => el.span,
		Node::Text(_, span) => span,
		Node::Expr(expr) => expr.span(),
	}
}

fn lower_widget(
	el: &Element,
	ctx: &mut LowerContext,
	diags: &mut Vec<Diagnostic>,
) -> Option<IrWidget> {
	let mut props = Vec::new();
	let mut event_props: Vec<(String, EventKind, Option<Expr>, Span)> = Vec::new();

	if let Some(schema) = schema_for(&el.name) {
		for attr in &el.attrs {
			if let Some(def) = schema.props.iter().find(|p| p.name == attr.name) {
				match &def.kind {
					crate::wui::compiler::registry::PropKind::Event(kind) => {
						let action_name = match &attr.value {
							AttrValue::String(name, _) => name.clone(),
							_ => continue,
						};
						let scoped = ctx.scoped_action(&action_name);
						let arg = get_expr_like(el, "arg");
						event_props.push((scoped, kind.clone(), arg, attr.span));
					}
					crate::wui::compiler::registry::PropKind::Bind(_) => {
						if let AttrValue::Expr(expr) = &attr.value {
							let prop_name = normalize_prop_name(&el.name, &attr.name);
							props.push(IrProp::Bind {
								name: prop_name,
								expr: expr.clone(),
							});
						}
					}
					crate::wui::compiler::registry::PropKind::Value(_) => {
						if attr.name == "arg" {
							continue;
						}
						let prop_name = normalize_prop_name(&el.name, &attr.name);
						lower_value_prop(&mut props, prop_name, &attr.value);
					}
				}
			} else if is_custom_component_tag(&el.name) {
				if let Some(event_name) = custom_event_name(&attr.name) {
					let action_name = match &attr.value {
						AttrValue::String(name, _) => name.clone(),
						_ => continue,
					};
					let scoped = ctx.scoped_action(&action_name);
					event_props.push((scoped, EventKind::Custom(event_name), None, attr.span));
				}
			}
		}
	} else {
		for attr in &el.attrs {
			if let Some(event_name) = custom_event_name(&attr.name) {
				let action_name = match &attr.value {
					AttrValue::String(name, _) => name.clone(),
					_ => continue,
				};
				let scoped = ctx.scoped_action(&action_name);
				event_props.push((scoped, EventKind::Custom(event_name), None, attr.span));
			} else {
				let prop_name = attr.name.clone();
				lower_value_prop(&mut props, prop_name, &attr.value);
			}
		}
	}

	for (action, kind, arg, span) in event_props {
		let payload = match &kind {
			EventKind::Click => {
				if arg.is_some() {
					ActionPayload::U32
				} else {
					ActionPayload::None
				}
			}
			EventKind::TextChanged => ActionPayload::String,
			EventKind::SliderChange => {
				if arg.is_some() {
					ActionPayload::U32I32
				} else {
					ActionPayload::I32
				}
			}
			EventKind::Select => ActionPayload::String,
			EventKind::Custom(_) => ActionPayload::Json,
		};
		let event_name = kind_name(&kind);
		ctx.add_action(action.clone(), kind, payload, span, diags);
		props.push(IrProp::Event {
			name: event_name,
			action,
			arg,
		});
	}

	let children = lower_nodes(&el.children, ctx, diags);
	Some(IrWidget {
		tag: el.name.clone(),
		props,
		children,
	})
}

fn lower_value_prop(props: &mut Vec<IrProp>, prop_name: String, value: &AttrValue) {
	match value {
		AttrValue::String(value, _) => props.push(IrProp::Literal {
			name: prop_name,
			value: value.clone(),
		}),
		AttrValue::Number(value, _) => props.push(IrProp::Number {
			name: prop_name,
			value: *value,
		}),
		AttrValue::Bool(value, _) => props.push(IrProp::Bool {
			name: prop_name,
			value: *value,
		}),
		AttrValue::Expr(expr) => props.push(IrProp::Value {
			name: prop_name,
			expr: expr.clone(),
		}),
		AttrValue::Null(_) => {}
	}
}

fn kind_name(kind: &EventKind) -> String {
	match kind {
		EventKind::Click => "onClick".to_string(),
		EventKind::TextChanged => "onTextChanged".to_string(),
		EventKind::SliderChange => "onSliderChange".to_string(),
		EventKind::Select => "onSelect".to_string(),
		EventKind::Custom(name) => name.clone(),
	}
}

fn is_custom_component_tag(name: &str) -> bool {
	matches!(name, "Custom" | "CustomComponent")
}

fn custom_event_name(name: &str) -> Option<String> {
	let rest = name.strip_prefix("on")?;
	let mut chars = rest.chars();
	let first = chars.next()?;
	if !first.is_ascii_uppercase() {
		return None;
	}
	let mut event = String::new();
	event.extend(first.to_lowercase());
	event.extend(chars);
	Some(event)
}

fn get_string_attr(el: &Element, name: &str) -> Option<String> {
	for attr in &el.attrs {
		if attr.name == name {
			if let AttrValue::String(value, _) = &attr.value {
				return Some(value.clone());
			}
		}
	}
	None
}

fn get_expr_attr(el: &Element, name: &str) -> Option<Expr> {
	for attr in &el.attrs {
		if attr.name == name {
			if let AttrValue::Expr(expr) = &attr.value {
				return Some(expr.clone());
			}
		}
	}
	None
}

fn get_expr_like(el: &Element, name: &str) -> Option<Expr> {
	for attr in &el.attrs {
		if attr.name == name {
			match &attr.value {
				AttrValue::Expr(expr) => return Some(expr.clone()),
				AttrValue::Number(value, span) => {
					return Some(Expr::Literal(
						crate::wui::ast::Literal::Number(*value),
						*span,
					));
				}
				_ => {}
			}
		}
	}
	None
}

fn normalize_prop_name(tag: &str, name: &str) -> String {
	match (tag, name) {
		("TextInput", "value") => "svalue".to_string(),
		("TextInput", "bind:value") => "bind:svalue".to_string(),
		("DatePicker", "value") => "svalue".to_string(),
		("DatePicker", "bind:value") => "bind:svalue".to_string(),
		("Slider", "value") => "ivalue".to_string(),
		("Slider", "bind:value") => "bind:ivalue".to_string(),
		_ => name.to_string(),
	}
}

fn hash_action_id(input: &str) -> u32 {
	const FNV_OFFSET: u32 = 0x811c9dc5;
	const FNV_PRIME: u32 = 0x01000193;
	let mut hash = FNV_OFFSET;
	for byte in input.as_bytes() {
		hash ^= *byte as u32;
		hash = hash.wrapping_mul(FNV_PRIME);
	}
	if hash == 0 {
		1
	} else {
		hash
	}
}
