use crate::wui::ast::{BinaryOp, Expr, Literal, UnaryOp};
use crate::wui::compiler::ir::{
	ActionDef, ActionPayload, EventKind, IrDocument, IrFor, IrIf, IrNode, IrProp, IrScope,
	IrSwitch, IrWidget,
};
use std::collections::BTreeSet;

pub fn generate(doc: &IrDocument) -> String {
	let mut out = String::new();
	out.push_str("use wgui::*;\n");
	if let Some(page) = doc.pages.first() {
		let component_name = format!("{}", pascal_case(&page.module));
		out.push_str(&format!(
			"use crate::components::{}::{};\n",
			page.module, component_name
		));
	}
	out.push('\n');
	out.push_str("pub enum Action {\n");
	for action in &doc.actions {
		out.push_str(&format!(
			"\t{}{},\n",
			action_variant(&action.name),
			action_payload(action)
		));
	}
	out.push_str("}\n\n");
	out.push_str("pub fn decode(event: &wgui::ClientEvent) -> Option<Action> {\n");
	out.push_str("\tmatch event {\n");
	for action in &doc.actions {
		out.push_str(&decode_arm(action));
	}
	out.push_str("\t\t_ => None,\n\t}\n}\n\n");
	let state_type = doc
		.pages
		.first()
		.and_then(|page| page.state_type.clone())
		.unwrap_or_else(|| "State".to_string());
	let state_type_path = state_type_path(&state_type);
	let param_names = collect_param_names(&doc.nodes);
	out.push_str("#[derive(Clone, Default)]\n");
	out.push_str("struct __WuiParams {\n");
	for name in &param_names {
		out.push_str(&format!("\tpub {}: String,\n", name));
	}
	out.push_str("}\n\n");
	out.push_str("fn __wui_param_set(params: &mut __WuiParams, name: &str, value: &str) {\n");
	out.push_str("\tmatch name {\n");
	for name in &param_names {
		out.push_str(&format!(
			"\t\t{:?} => params.{} = value.to_string(),\n",
			name, name
		));
	}
	out.push_str("\t\t_ => {}\n\t}\n}\n\n");
	out.push_str(&format!(
		"pub fn render(state: &{}) -> Item {{\n",
		state_type_path
	));
	out.push_str("\trender_with_path(state, \"\")\n");
	out.push_str("}\n\n");
	out.push_str(&format!(
		"pub fn render_with_path(state: &{}, path: &str) -> Item {{\n",
		state_type_path
	));
	out.push_str("\tlet __path = path;\n");
	out.push_str(&emit_nodes(&doc.nodes, 1));
	out.push_str("}\n\n");
	out.push_str("fn __wui_route_params(route: &str, path: &str) -> Option<__WuiParams> {\n");
	out.push_str("\tif route == path { return Some(__WuiParams::default()); }\n");
	out.push_str("\tlet route_parts: Vec<&str> = route.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();\n");
	out.push_str("\tlet path_parts: Vec<&str> = path.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();\n");
	out.push_str("\tlet mut params = __WuiParams::default();\n");
	out.push_str("\tlet mut wildcard_at = None;\n");
	out.push_str("\tfor (index, seg) in route_parts.iter().enumerate() {\n");
	out.push_str("\t\tif *seg == \"*\" || *seg == \"{*wildcard}\" {\n");
	out.push_str("\t\t\twildcard_at = Some(index);\n");
	out.push_str("\t\t\tbreak;\n");
	out.push_str("\t\t}\n");
	out.push_str("\t}\n");
	out.push_str("\tlet end = wildcard_at.unwrap_or(route_parts.len());\n");
	out.push_str("\tif wildcard_at.is_none() && end != path_parts.len() { return None; }\n");
	out.push_str("\tif wildcard_at.is_some() && path_parts.len() < end { return None; }\n");
	out.push_str("\tfor i in 0..end {\n");
	out.push_str("\t\tlet route_seg = route_parts[i];\n");
	out.push_str("\t\tlet path_seg = path_parts[i];\n");
	out.push_str("\t\tif let Some(name) = __wui_param_name(route_seg) {\n");
	out.push_str("\t\t\t__wui_param_set(&mut params, name, path_seg);\n");
	out.push_str("\t\t} else if route_seg != path_seg {\n");
	out.push_str("\t\t\treturn None;\n");
	out.push_str("\t\t}\n");
	out.push_str("\t}\n");
	out.push_str("\tSome(params)\n");
	out.push_str("}\n");
	out.push_str("fn __wui_param_name(segment: &str) -> Option<&str> {\n");
	out.push_str("\tif let Some(name) = segment.strip_prefix(':') {\n");
	out.push_str("\t\tif !name.is_empty() { return Some(name); }\n");
	out.push_str("\t}\n");
	out.push_str("\tif segment.starts_with('{') && segment.ends_with('}') {\n");
	out.push_str("\t\tlet inner = &segment[1..segment.len() - 1];\n");
	out.push_str("\t\tif inner.starts_with('*') { return None; }\n");
	out.push_str("\t\tif !inner.is_empty() { return Some(inner); }\n");
	out.push_str("\t}\n");
	out.push_str("\tNone\n");
	out.push_str("}\n");
	out
}

pub fn generate_controller_stub(doc: &IrDocument, module_name: &str) -> Option<String> {
	let state_type = doc
		.pages
		.first()
		.and_then(|page| page.state_type.clone())
		.unwrap_or_else(|| "State".to_string());
	let state_type_path = state_type_path(&state_type);
	let controller_name = format!("{}", pascal_case(module_name));
	let mut out = String::new();
	out.push_str(&format!(
		"pub struct {} {{\n\tpub state: {},\n}}\n\n",
		controller_name, state_type_path
	));
	out.push_str(&format!(
		"impl {} {{\n\tpub fn new(state: {}) -> Self {{\n\t\tSelf {{ state }}\n\t}}\n\n",
		controller_name, state_type_path
	));
	for action in &doc.actions {
		let method = action_method_name(&action.name);
		match action.payload {
			ActionPayload::None => {
				out.push_str(&format!(
					"\tpub(crate) fn {}(&mut self) {{\n\t\t// TODO\n\t}}\n\n",
					method
				));
			}
			ActionPayload::U32 => {
				out.push_str(&format!(
					"\tpub(crate) fn {}(&mut self, _arg: u32) {{\n\t\t// TODO\n\t}}\n\n",
					method
				));
			}
			ActionPayload::String => {
				out.push_str(&format!(
					"\tpub(crate) fn {}(&mut self, _value: String) {{\n\t\t// TODO\n\t}}\n\n",
					method
				));
			}
			ActionPayload::I32 => {
				out.push_str(&format!(
					"\tpub(crate) fn {}(&mut self, _value: i32) {{\n\t\t// TODO\n\t}}\n\n",
					method
				));
			}
			ActionPayload::U32I32 => {
				out.push_str(&format!(
					"\tpub(crate) fn {}(&mut self, _arg: u32, _value: i32) {{\n\t\t// TODO\n\t}}\n\n",
					method
				));
			}
			ActionPayload::Json => {
				out.push_str(&format!(
					"\tpub(crate) fn {}(&mut self, _payload: wgui::serde_json::Value) {{\n\t\t// TODO\n\t}}\n\n",
					method
				));
			}
		}
	}
	out.push_str("}\n");
	Some(out)
}

fn action_payload(action: &ActionDef) -> String {
	match action.payload {
		ActionPayload::None => String::new(),
		ActionPayload::U32 => " { arg: u32 }".to_string(),
		ActionPayload::String => " { value: String }".to_string(),
		ActionPayload::I32 => " { value: i32 }".to_string(),
		ActionPayload::U32I32 => " { arg: u32, value: i32 }".to_string(),
		ActionPayload::Json => " { payload: wgui::serde_json::Value }".to_string(),
	}
}

fn decode_arm(action: &ActionDef) -> String {
	let variant = action_variant(&action.name);
	let id = action.id;
	match &action.kind {
		EventKind::Click => match action.payload {
			ActionPayload::None => format!(
				"\t\twgui::ClientEvent::OnClick(ev) if ev.id == {id} => Some(Action::{variant}),\n"
			),
			ActionPayload::U32 => format!(
				"\t\twgui::ClientEvent::OnClick(ev) if ev.id == {id} => ev.inx.map(|arg| Action::{variant} {{ arg }}),\n"
			),
			_ => String::new(),
		},
		EventKind::TextChanged => format!(
			"\t\twgui::ClientEvent::OnTextChanged(ev) if ev.id == {id} => Some(Action::{variant} {{ value: ev.value.clone() }}),\n"
		),
		EventKind::SliderChange => match action.payload {
			ActionPayload::I32 => format!(
				"\t\twgui::ClientEvent::OnSliderChange(ev) if ev.id == {id} => Some(Action::{variant} {{ value: ev.value }}),\n"
			),
			ActionPayload::U32I32 => format!(
				"\t\twgui::ClientEvent::OnSliderChange(ev) if ev.id == {id} => ev.inx.map(|arg| Action::{variant} {{ arg, value: ev.value }}),\n"
			),
			_ => String::new(),
		},
		EventKind::Select => format!(
			"\t\twgui::ClientEvent::OnSelect(ev) if ev.id == {id} => Some(Action::{variant} {{ value: ev.value.clone() }}),\n"
		),
		EventKind::Custom(event_name) => format!(
			"\t\twgui::ClientEvent::OnCustom(ev) if ev.id == {id} && ev.name == {:?} => Some(Action::{variant} {{ payload: ev.payload.clone() }}),\n",
			event_name
		),
	}
}

fn emit_nodes(nodes: &[IrNode], indent: usize) -> String {
	let mut out = String::new();
	let indent_str = "\t".repeat(indent);
	out.push_str(&format!("{indent_str}let mut children = Vec::new();\n"));
	for node in nodes {
		out.push_str(&emit_node_into(node, indent, "children"));
	}
	out.push_str(&format!("{indent_str}wgui::vstack(children)\n"));
	out
}

fn emit_node_into(node: &IrNode, indent: usize, target: &str) -> String {
	let indent_str = "\t".repeat(indent);
	match node {
		IrNode::Widget(widget) => {
			let rendered = emit_widget(widget, indent + 1);
			format!("{indent_str}{target}.push({rendered});\n")
		}
		IrNode::Text(text) => format!("{indent_str}{target}.push(wgui::text({:?}));\n", text),
		IrNode::For(for_node) => emit_for(for_node, indent, target),
		IrNode::If(if_node) => emit_if(if_node, indent, target),
		IrNode::Scope(scope) => emit_scope(scope, indent, target),
		IrNode::Route(route) => emit_route(route, indent, target),
		IrNode::Switch(node) => emit_switch(node, indent, target),
		IrNode::Children => String::new(),
	}
}

fn emit_for(node: &IrFor, indent: usize, target: &str) -> String {
	let indent_str = "\t".repeat(indent);
	let mut out = String::new();
	let list_expr = emit_expr(&node.each);
	let item = &node.item;
	if let Some(index) = &node.index {
		out.push_str(&format!(
			"{indent_str}for ({index}, {item}) in {list_expr}.iter().enumerate() {{\n"
		));
	} else {
		out.push_str(&format!(
			"{indent_str}for {item} in {list_expr}.iter() {{\n"
		));
	}
	out.push_str(&emit_body(&node.body, indent + 1, target));
	out.push_str(&format!("{indent_str}}}\n"));
	out
}

fn emit_if(node: &IrIf, indent: usize, target: &str) -> String {
	let indent_str = "\t".repeat(indent);
	let mut out = String::new();
	let test = emit_expr(&node.test);
	out.push_str(&format!("{indent_str}if {test} {{\n"));
	out.push_str(&emit_body(&node.then_body, indent + 1, target));
	out.push_str(&format!("{indent_str}}}"));
	if !node.else_body.is_empty() {
		out.push_str(" else {\n");
		out.push_str(&emit_body(&node.else_body, indent + 1, target));
		out.push_str(&format!("{indent_str}}}\n"));
	} else {
		out.push('\n');
	}
	out
}

fn emit_scope(node: &IrScope, indent: usize, target: &str) -> String {
	emit_body(&node.body, indent, target)
}

fn emit_route(node: &crate::wui::compiler::ir::IrRoute, indent: usize, target: &str) -> String {
	let indent_str = "\t".repeat(indent);
	let mut out = String::new();
	out.push_str(&format!(
		"{indent_str}if let Some(params) = __wui_route_params({:?}, __path) {{\n",
		node.path
	));
	out.push_str(&format!("{indent_str}\tlet _ = &params;\n"));
	out.push_str(&emit_body(&node.body, indent + 1, target));
	out.push_str(&format!("{indent_str}}}\n"));
	out
}

fn emit_switch(node: &IrSwitch, indent: usize, target: &str) -> String {
	let indent_str = "\t".repeat(indent);
	let mut out = String::new();
	let mut first = true;
	for case in &node.cases {
		if first {
			out.push_str(&format!(
				"{indent_str}if let Some(params) = __wui_route_params({:?}, __path) {{\n",
				case.path
			));
			out.push_str(&format!("{indent_str}\tlet _ = &params;\n"));
			out.push_str(&emit_body(&case.body, indent + 1, target));
			out.push_str(&format!("{indent_str}}}"));
			first = false;
		} else {
			out.push_str(&format!(
				" else if let Some(params) = __wui_route_params({:?}, __path) {{\n",
				case.path
			));
			out.push_str(&format!("{indent_str}\tlet _ = &params;\n"));
			out.push_str(&emit_body(&case.body, indent + 1, target));
			out.push_str(&format!("{indent_str}}}"));
		}
	}
	if !out.is_empty() {
		out.push('\n');
	}
	out
}

fn collect_param_names(nodes: &[IrNode]) -> Vec<String> {
	let mut names = BTreeSet::new();
	collect_param_names_into(nodes, &mut names);
	names.into_iter().collect()
}

fn collect_param_names_into(nodes: &[IrNode], out: &mut BTreeSet<String>) {
	for node in nodes {
		match node {
			IrNode::Widget(widget) => collect_param_names_into(&widget.children, out),
			IrNode::For(node) => collect_param_names_into(&node.body, out),
			IrNode::If(node) => {
				collect_param_names_into(&node.then_body, out);
				collect_param_names_into(&node.else_body, out);
			}
			IrNode::Scope(node) => collect_param_names_into(&node.body, out),
			IrNode::Route(node) => collect_names_from_route(&node.path, out),
			IrNode::Switch(node) => {
				for case in &node.cases {
					collect_names_from_route(&case.path, out);
					collect_param_names_into(&case.body, out);
				}
			}
			IrNode::Children | IrNode::Text(_) => {}
		}
	}
}

fn collect_names_from_route(route: &str, out: &mut BTreeSet<String>) {
	for segment in route.split('/').filter(|seg| !seg.is_empty()) {
		if let Some(name) = segment.strip_prefix(':') {
			if !name.is_empty() {
				out.insert(name.to_string());
			}
		} else if segment.starts_with('{') && segment.ends_with('}') && !segment.starts_with("{*") {
			let name = &segment[1..segment.len() - 1];
			if !name.is_empty() {
				out.insert(name.to_string());
			}
		}
	}
}

fn emit_body(nodes: &[IrNode], indent: usize, target: &str) -> String {
	let mut out = String::new();
	for node in nodes {
		out.push_str(&emit_node_into(node, indent, target));
	}
	out
}

fn emit_widget(widget: &IrWidget, indent: usize) -> String {
	let mut base = match widget.tag.as_str() {
		"VStack" => emit_container("vstack", &widget.children, indent),
		"HStack" => emit_container("hstack", &widget.children, indent),
		"Text" => emit_text(widget),
		"Button" => emit_textual("button", widget, "text"),
		"Link" => emit_link(widget),
		"TextInput" => "wgui::text_input()".to_string(),
		"DatePicker" => "wgui::date_picker()".to_string(),
		"Checkbox" => "wgui::checkbox()".to_string(),
		"Slider" => "wgui::slider()".to_string(),
		"Image" => emit_image(widget),
		"Video" => emit_media(widget, "video"),
		"Audio" => emit_media(widget, "audio"),
		"FolderPicker" => "wgui::folder_picker()".to_string(),
		"Modal" => emit_container("modal", &widget.children, indent),
		_ => "wgui::text(\"unsupported\")".to_string(),
	};
	for prop in &widget.props {
		if !should_emit_prop(&widget.tag, prop) {
			continue;
		}
		base = format!("{}.{}", base, emit_prop(prop));
	}
	base
}

fn emit_container(kind: &str, children: &[IrNode], indent: usize) -> String {
	let mut out = String::new();
	let indent_str = "\t".repeat(indent);
	out.push_str("{\n");
	out.push_str(&format!("{indent_str}let mut items = Vec::new();\n"));
	for node in children {
		out.push_str(&emit_container_child(node, indent, "items"));
	}
	out.push_str(&format!("{indent_str}wgui::{kind}(items)\n"));
	out.push_str(&format!("{indent_str}}}"));
	out
}

fn emit_container_child(node: &IrNode, indent: usize, target: &str) -> String {
	emit_node_into(node, indent, target)
}

fn emit_text(widget: &IrWidget) -> String {
	for prop in &widget.props {
		if let IrProp::Literal { name, value } = prop {
			if name == "value" {
				return format!("wgui::text({:?})", value);
			}
		}
		if let IrProp::Value { name, expr } = prop {
			if name == "value" {
				return format!("wgui::text({})", emit_string_expr(expr));
			}
		}
	}
	"wgui::text(\"\")".to_string()
}

fn emit_textual(kind: &str, widget: &IrWidget, prop_name: &str) -> String {
	for prop in &widget.props {
		if let IrProp::Literal { name, value } = prop {
			if name == prop_name {
				return format!("wgui::{kind}({:?})", value);
			}
		}
		if let IrProp::Value { name, expr } = prop {
			if name == prop_name {
				return format!("wgui::{kind}({})", emit_string_expr(expr));
			}
		}
	}
	format!("wgui::{kind}(\"\")")
}

fn emit_link(widget: &IrWidget) -> String {
	let mut href = "\"\"".to_string();
	let mut text = "\"\"".to_string();
	for prop in &widget.props {
		match prop {
			IrProp::Literal { name, value } if name == "href" => href = format!("{:?}", value),
			IrProp::Value { name, expr } if name == "href" => href = emit_string_expr(expr),
			IrProp::Literal { name, value } if name == "text" => text = format!("{:?}", value),
			IrProp::Value { name, expr } if name == "text" => text = emit_string_expr(expr),
			_ => {}
		}
	}
	format!("wgui::link({href}, {text})")
}

fn emit_image(widget: &IrWidget) -> String {
	let mut src = "\"\"".to_string();
	let mut alt = "\"\"".to_string();
	for prop in &widget.props {
		match prop {
			IrProp::Literal { name, value } if name == "src" => src = format!("{:?}", value),
			IrProp::Value { name, expr } if name == "src" => src = emit_string_expr(expr),
			IrProp::Literal { name, value } if name == "alt" => alt = format!("{:?}", value),
			IrProp::Value { name, expr } if name == "alt" => alt = emit_string_expr(expr),
			_ => {}
		}
	}
	format!("wgui::img({src}, {alt})")
}

fn emit_media(widget: &IrWidget, kind: &str) -> String {
	let mut room = "\"\"".to_string();
	for prop in &widget.props {
		match prop {
			IrProp::Literal { name, value } if name == "room" => room = format!("{:?}", value),
			IrProp::Value { name, expr } if name == "room" => room = emit_string_expr(expr),
			_ => {}
		}
	}
	format!("wgui::{kind}({room})")
}

fn emit_prop(prop: &IrProp) -> String {
	match prop {
		IrProp::Literal { name, value } => format!("{}({:?})", prop_method(name), value),
		IrProp::Number { name, value } => emit_number_prop(name, *value),
		IrProp::Bool { name, value } => format!("{}({})", prop_method(name), value),
		IrProp::Value { name, expr } => match name.as_str() {
			"svalue" => format!("svalue({})", emit_string_expr(expr)),
			"ivalue" => format!("ivalue({})", emit_expr(expr)),
			"checked" => format!("checked({})", emit_expr(expr)),
			_ if is_string_prop(name) => {
				format!("{}({})", prop_method(name), emit_string_expr(expr))
			}
			_ => format!("{}({})", prop_method(name), emit_expr(expr)),
		},
		IrProp::Bind { name, expr } => match name.as_str() {
			"bind:svalue" => format!("svalue({})", emit_string_expr(expr)),
			"bind:ivalue" => format!("ivalue({})", emit_expr(expr)),
			"bind:checked" => format!("checked({})", emit_expr(expr)),
			_ => String::new(),
		},
		IrProp::Event { name, action, arg } => {
			if !name.starts_with("on") {
				return format!("custom_event({:?}, {})", name, action_id(action));
			}
			let mut base = format!("id({})", action_id(action));
			if let Some(expr) = arg {
				base = format!("{base}.inx({})", emit_expr(expr));
			}
			base
		}
	}
}

fn emit_number_prop(name: &str, value: f64) -> String {
	match name {
		"min" | "max" | "step" | "ivalue" => format!("{}({})", prop_method(name), value as i32),
		"padding" | "paddingLeft" | "paddingRight" | "paddingTop" | "paddingBottom" | "margin"
		| "marginLeft" | "marginRight" | "marginTop" | "marginBottom" => {
			format!("{}({})", prop_method(name), value as u16)
		}
		_ => format!("{}({})", prop_method(name), value as u32),
	}
}

fn prop_method(name: &str) -> String {
	match name {
		"ivalue" => "ivalue".to_string(),
		"spacing" => "spacing".to_string(),
		"padding" => "padding".to_string(),
		"paddingLeft" => "padding_left".to_string(),
		"paddingRight" => "padding_right".to_string(),
		"paddingTop" => "padding_top".to_string(),
		"paddingBottom" => "padding_bottom".to_string(),
		"margin" => "margin".to_string(),
		"marginLeft" => "margin_left".to_string(),
		"marginRight" => "margin_right".to_string(),
		"marginTop" => "margin_top".to_string(),
		"marginBottom" => "margin_bottom".to_string(),
		"backgroundColor" => "background_color".to_string(),
		"color" => "color".to_string(),
		"border" => "border".to_string(),
		"width" => "width".to_string(),
		"height" => "height".to_string(),
		"minWidth" => "min_width".to_string(),
		"maxWidth" => "max_width".to_string(),
		"minHeight" => "min_height".to_string(),
		"maxHeight" => "max_height".to_string(),
		"grow" => "grow".to_string(),
		"textAlign" => "text_align".to_string(),
		"cursor" => "cursor".to_string(),
		"breakWords" => "break_words".to_string(),
		"fill" => "fill".to_string(),
		"wrap" => "wrap".to_string(),
		"overflow" => "overflow".to_string(),
		"placeholder" => "placeholder".to_string(),
		"objectFit" => "object_fit".to_string(),
		"room" => "room".to_string(),
		"local" => "local".to_string(),
		"autoplay" => "autoplay".to_string(),
		"muted" => "muted".to_string(),
		"controls" => "controls".to_string(),
		"min" => "min".to_string(),
		"max" => "max".to_string(),
		"step" => "step".to_string(),
		_ => name.to_string(),
	}
}

fn emit_expr(expr: &Expr) -> String {
	match expr {
		Expr::Literal(lit, _) => match lit {
			Literal::String(s) => format!("{:?}", s),
			Literal::Number(n) => format!("{}", n),
			Literal::Bool(b) => format!("{}", b),
			Literal::Null => "None".to_string(),
		},
		Expr::Path(parts, _) => parts.join("."),
		Expr::Call { name, args, .. } => emit_call_expr(name, args),
		Expr::Unary { op, expr, .. } => match op {
			UnaryOp::Not => format!("!{}", emit_expr(expr)),
			UnaryOp::Neg => format!("-{}", emit_expr(expr)),
		},
		Expr::Binary {
			left, op, right, ..
		} => {
			let op_str = match op {
				BinaryOp::Add => "+",
				BinaryOp::Sub => "-",
				BinaryOp::Mul => "*",
				BinaryOp::Div => "/",
				BinaryOp::Mod => "%",
				BinaryOp::Eq => "==",
				BinaryOp::Neq => "!=",
				BinaryOp::Lt => "<",
				BinaryOp::Lte => "<=",
				BinaryOp::Gt => ">",
				BinaryOp::Gte => ">=",
				BinaryOp::And => "&&",
				BinaryOp::Or => "||",
			};
			format!("({} {} {})", emit_expr(left), op_str, emit_expr(right))
		}
		Expr::Ternary {
			cond,
			then_expr,
			else_expr,
			..
		} => format!(
			"if {} {{ {} }} else {{ {} }}",
			emit_expr(cond),
			emit_expr(then_expr),
			emit_expr(else_expr)
		),
		Expr::Coalesce { left, right, .. } => {
			format!(
				"{}.unwrap_or_else(|| {})",
				emit_expr(left),
				emit_expr(right)
			)
		}
	}
}

fn emit_call_expr(name: &str, args: &[Expr]) -> String {
	match (name, args) {
		("path_matches", [pattern]) => {
			format!(
				"__wui_route_params({}, __path).is_some()",
				emit_string_expr(pattern)
			)
		}
		_ => "None".to_string(),
	}
}

fn emit_string_expr(expr: &Expr) -> String {
	match expr {
		Expr::Path(_, _) => format!("&{}", emit_expr(expr)),
		_ => emit_expr(expr),
	}
}

fn is_string_prop(name: &str) -> bool {
	matches!(
		name,
		"textAlign"
			| "cursor"
			| "overflow"
			| "placeholder"
			| "backgroundColor"
			| "color" | "border"
			| "objectFit"
			| "room"
	)
}

fn should_emit_prop(tag: &str, prop: &IrProp) -> bool {
	match prop {
		IrProp::Event { .. } => true,
		IrProp::Literal { name, .. }
		| IrProp::Number { name, .. }
		| IrProp::Bool { name, .. }
		| IrProp::Value { name, .. }
		| IrProp::Bind { name, .. } => match tag {
			"Text" => name != "value",
			"Button" => name != "text",
			"Link" => name != "href" && name != "text",
			"Image" => name != "src" && name != "alt",
			"Video" | "Audio" => name != "room",
			_ => name != "arg",
		},
	}
}

fn state_type_path(state_type: &str) -> String {
	if state_type.contains("::")
		|| state_type.starts_with("crate::")
		|| state_type.starts_with("super::")
	{
		state_type.to_string()
	} else {
		format!("crate::{}", state_type)
	}
}

fn action_variant(name: &str) -> String {
	let mut out = String::new();
	let mut upper_next = true;
	for ch in name.chars() {
		if ch.is_ascii_alphanumeric() {
			if upper_next {
				out.push(ch.to_ascii_uppercase());
				upper_next = false;
			} else {
				out.push(ch);
			}
		} else {
			upper_next = true;
		}
	}
	if out.is_empty() {
		"Action".to_string()
	} else {
		out
	}
}

fn action_id(name: &str) -> u32 {
	let mut hash = 0x811c9dc5u32;
	for byte in name.as_bytes() {
		hash ^= *byte as u32;
		hash = hash.wrapping_mul(0x01000193);
	}
	if hash == 0 {
		1
	} else {
		hash
	}
}

fn pascal_case(input: &str) -> String {
	let mut out = String::new();
	let mut upper_next = true;
	for ch in input.chars() {
		if ch.is_ascii_alphanumeric() {
			if upper_next {
				out.push(ch.to_ascii_uppercase());
				upper_next = false;
			} else {
				out.push(ch);
			}
		} else {
			upper_next = true;
		}
	}
	if out.is_empty() {
		"Controller".to_string()
	} else {
		out
	}
}

fn action_method_name(name: &str) -> String {
	let mut out = String::new();
	let mut prev_underscore = false;
	for (i, ch) in name.chars().enumerate() {
		if ch.is_ascii_alphanumeric() {
			if ch.is_ascii_uppercase() {
				if i != 0 && !prev_underscore {
					out.push('_');
				}
				out.push(ch.to_ascii_lowercase());
				prev_underscore = false;
			} else {
				out.push(ch.to_ascii_lowercase());
				prev_underscore = false;
			}
		} else if !prev_underscore {
			out.push('_');
			prev_underscore = true;
		}
	}
	if out.ends_with('_') {
		out.pop();
	}
	if out.is_empty() {
		"action".to_string()
	} else {
		out
	}
}
