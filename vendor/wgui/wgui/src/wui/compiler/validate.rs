use crate::wui::ast::{AttrValue, Expr, Literal, Node};
use crate::wui::compiler::registry::{is_structural, schema_for, PropKind, ValueType};
use crate::wui::diagnostic::Diagnostic;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ValidatedDocument {
	pub nodes: Vec<Node>,
	pub components: HashMap<String, Vec<Node>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExprType {
	String,
	Number,
	Bool,
	Null,
	Unknown,
}

pub fn validate(
	nodes: &[Node],
	components: &HashMap<String, Vec<Node>>,
	diags: &mut Vec<Diagnostic>,
) -> Option<ValidatedDocument> {
	for node in nodes {
		validate_node(node, components, diags);
	}
	for nodes in components.values() {
		for node in nodes {
			validate_node(node, components, diags);
		}
	}
	if diags.is_empty() {
		Some(ValidatedDocument {
			nodes: nodes.to_vec(),
			components: components.clone(),
		})
	} else {
		None
	}
}

fn validate_node(
	node: &Node,
	components: &HashMap<String, Vec<Node>>,
	diags: &mut Vec<Diagnostic>,
) {
	match node {
		Node::Element(el) => {
			if is_structural(&el.name) {
				validate_structural(el, diags);
			} else if components.contains_key(&el.name) {
				validate_component_use(el, components, diags);
			} else {
				validate_widget(el, diags);
			}
			for child in &el.children {
				validate_node(child, components, diags);
			}
		}
		Node::Text(_, _) | Node::Expr(_) => {}
	}
}

fn validate_structural(el: &crate::wui::ast::Element, diags: &mut Vec<Diagnostic>) {
	match el.name.as_str() {
		"For" => {
			require_attr(el, "each", diags);
			allow_only(el, &["each", "itemAs", "indexAs", "key"], diags);
		}
		"If" => {
			require_attr(el, "test", diags);
			allow_only(el, &["test"], diags);
		}
		"Else" => {
			if !el.attrs.is_empty() {
				diags.push(Diagnostic::new("Else does not take attributes", el.span));
			}
		}
		"Scope" => {
			require_attr(el, "name", diags);
			allow_only(el, &["name"], diags);
		}
		"Page" => {
			diags.push(Diagnostic::new(
				"Page is deprecated; use Route instead",
				el.span,
			));
			if let Some((route, span)) = string_attr(el, "route") {
				validate_path_params(route, span, diags);
			}
			allow_only(el, &["route", "title", "state"], diags);
		}
		"Route" => {
			let has_path = el.attrs.iter().any(|attr| attr.name == "path");
			let has_route = el.attrs.iter().any(|attr| attr.name == "route");
			if !has_path && !has_route {
				diags.push(Diagnostic::new("Route requires a path attribute", el.span));
			}
			if has_path {
				require_string_attr(el, "path", diags);
				if let Some((path, span)) = string_attr(el, "path") {
					validate_path_params(path, span, diags);
				}
			}
			if has_route {
				require_string_attr(el, "route", diags);
				if let Some((route, span)) = string_attr(el, "route") {
					validate_path_params(route, span, diags);
				}
			}
			allow_only(el, &["path", "route", "title", "state"], diags);
		}
		"Switch" => {
			if !el.attrs.is_empty() {
				diags.push(Diagnostic::new("Switch does not take attributes", el.span));
			}
			for child in &el.children {
				match child {
					Node::Element(child_el) if child_el.name == "Case" => {}
					_ => {
						diags.push(Diagnostic::new("Switch only allows Case children", el.span));
					}
				}
			}
		}
		"Case" => {
			let has_path = el.attrs.iter().any(|attr| attr.name == "path");
			let has_route = el.attrs.iter().any(|attr| attr.name == "route");
			if !has_path && !has_route {
				diags.push(Diagnostic::new("Case requires a path attribute", el.span));
			}
			if has_path {
				require_string_attr(el, "path", diags);
				if let Some((path, span)) = string_attr(el, "path") {
					validate_path_params(path, span, diags);
				}
			}
			if has_route {
				require_string_attr(el, "route", diags);
				if let Some((route, span)) = string_attr(el, "route") {
					validate_path_params(route, span, diags);
				}
			}
			allow_only(el, &["path", "route", "title", "state"], diags);
		}
		"Import" => {
			if el.attrs.iter().any(|attr| attr.name == "name") {
				require_attr(el, "name", diags);
				require_string_attr(el, "name", diags);
				if el.attrs.iter().any(|attr| attr.name == "from") {
					require_string_attr(el, "from", diags);
				} else {
					require_attr(el, "src", diags);
					require_string_attr(el, "src", diags);
				}
				allow_only(el, &["name", "from", "src"], diags);
			} else {
				require_attr(el, "src", diags);
				require_string_attr(el, "src", diags);
				allow_only(el, &["src"], diags);
			}
			if !el.children.is_empty() {
				diags.push(Diagnostic::new("Import does not take children", el.span));
			}
		}
		"Children" => {
			if !el.attrs.is_empty() {
				diags.push(Diagnostic::new(
					"Children does not take attributes",
					el.span,
				));
			}
			if !el.children.is_empty() {
				diags.push(Diagnostic::new("Children does not take children", el.span));
			}
		}
		_ => {}
	}
}

fn validate_component_use(
	el: &crate::wui::ast::Element,
	components: &HashMap<String, Vec<Node>>,
	diags: &mut Vec<Diagnostic>,
) {
	let Some(component_nodes) = components.get(&el.name) else {
		return;
	};
	let accepts_children = component_uses_children(component_nodes);
	if !accepts_children && !el.children.is_empty() {
		diags.push(Diagnostic::new(
			format!("{} does not render Children", el.name),
			el.span,
		));
	}
	for attr in &el.attrs {
		if is_custom_event_attr(&attr.name) {
			if !matches!(attr.value, AttrValue::String(_, _)) {
				diags.push(Diagnostic::new(
					"event handlers must be string literals",
					attr.span,
				));
			}
			continue;
		}
		if matches!(attr.value, AttrValue::Null(_)) {
			diags.push(Diagnostic::new(
				format!("invalid value for {}", attr.name),
				attr.span,
			));
		}
	}
	if el.attrs.iter().any(|attr| is_custom_event_attr(&attr.name))
		&& !component_is_single_custom_root(component_nodes)
	{
		diags.push(Diagnostic::new(
			format!(
				"{} custom event bindings require a single CustomComponent root",
				el.name
			),
			el.span,
		));
	}
}

fn component_uses_children(nodes: &[Node]) -> bool {
	for node in nodes {
		let Node::Element(el) = node else {
			continue;
		};
		if el.name == "Children" || component_uses_children(&el.children) {
			return true;
		}
	}
	false
}

fn validate_widget(el: &crate::wui::ast::Element, diags: &mut Vec<Diagnostic>) {
	let Some(schema) = schema_for(&el.name) else {
		diags.push(Diagnostic::new("unknown tag", el.span));
		return;
	};
	let mut event_count = 0;
	for attr in &el.attrs {
		let Some(prop) = schema.props.iter().find(|p| p.name == attr.name) else {
			if is_custom_component_tag(&el.name) && is_custom_event_attr(&attr.name) {
				if !matches!(attr.value, AttrValue::String(_, _)) {
					diags.push(Diagnostic::new(
						"event handlers must be string literals",
						attr.span,
					));
				}
				continue;
			}
			diags.push(Diagnostic::new(
				format!("unknown prop {}", attr.name),
				attr.span,
			));
			continue;
		};
		match prop.kind {
			PropKind::Event(_) => {
				event_count += 1;
				if !matches!(attr.value, AttrValue::String(_, _)) {
					diags.push(Diagnostic::new(
						"event handlers must be string literals",
						attr.span,
					));
				}
			}
			PropKind::Bind(value_type) | PropKind::Value(value_type) => {
				if !attr_value_matches(&attr.value, value_type) {
					diags.push(Diagnostic::new(
						format!("invalid value for {}", attr.name),
						attr.span,
					));
				}
			}
		}
	}
	if event_count > 1 {
		diags.push(Diagnostic::new(
			"only one event handler per element is supported",
			el.span,
		));
	}
	check_bind_conflicts(el, diags);
}

fn component_is_single_custom_root(nodes: &[Node]) -> bool {
	matches!(
		nodes,
		[Node::Element(el)] if matches!(el.name.as_str(), "Custom" | "CustomComponent")
	)
}

fn is_custom_component_tag(name: &str) -> bool {
	matches!(name, "Custom" | "CustomComponent")
}

fn is_custom_event_attr(name: &str) -> bool {
	let Some(rest) = name.strip_prefix("on") else {
		return false;
	};
	rest.chars()
		.next()
		.map(|ch| ch.is_ascii_uppercase())
		.unwrap_or(false)
}

fn check_bind_conflicts(el: &crate::wui::ast::Element, diags: &mut Vec<Diagnostic>) {
	let mut has_value = false;
	let mut has_bind = false;
	for attr in &el.attrs {
		if attr.name == "value" {
			has_value = true;
		}
		if attr.name.starts_with("bind:") {
			has_bind = true;
		}
	}
	if has_value && has_bind {
		diags.push(Diagnostic::new(
			"cannot use value with bind:* on same element",
			el.span,
		));
	}
}

fn require_attr(el: &crate::wui::ast::Element, name: &str, diags: &mut Vec<Diagnostic>) {
	if !el.attrs.iter().any(|attr| attr.name == name) {
		diags.push(Diagnostic::new(
			format!("missing required attribute {}", name),
			el.span,
		));
	}
}

fn require_string_attr(el: &crate::wui::ast::Element, name: &str, diags: &mut Vec<Diagnostic>) {
	let Some(attr) = el.attrs.iter().find(|attr| attr.name == name) else {
		return;
	};
	if !matches!(attr.value, AttrValue::String(_, _)) {
		diags.push(Diagnostic::new(
			format!("{} must be a string literal", name),
			attr.span,
		));
	}
}

fn string_attr<'a>(
	el: &'a crate::wui::ast::Element,
	name: &str,
) -> Option<(&'a str, crate::wui::diagnostic::Span)> {
	let attr = el.attrs.iter().find(|attr| attr.name == name)?;
	match &attr.value {
		AttrValue::String(value, _) => Some((value.as_str(), attr.span)),
		_ => None,
	}
}

fn validate_path_params(
	path: &str,
	span: crate::wui::diagnostic::Span,
	diags: &mut Vec<Diagnostic>,
) {
	for segment in path.split('/').filter(|seg| !seg.is_empty()) {
		if segment == "*" || segment == "{*wildcard}" {
			continue;
		}
		if let Some(name) = segment.strip_prefix(':') {
			if !is_valid_ident(name) {
				diags.push(Diagnostic::new(
					format!("invalid path parameter name {}", name),
					span,
				));
			}
			continue;
		}
		if segment.starts_with('{') && segment.ends_with('}') {
			let inner = &segment[1..segment.len() - 1];
			if inner.starts_with('*') {
				continue;
			}
			if !is_valid_ident(inner) {
				diags.push(Diagnostic::new(
					format!("invalid path parameter name {}", inner),
					span,
				));
			}
		}
	}
}

fn is_valid_ident(name: &str) -> bool {
	let mut chars = name.chars();
	let Some(first) = chars.next() else {
		return false;
	};
	if !(first == '_' || first.is_ascii_alphabetic()) {
		return false;
	}
	for ch in chars {
		if !(ch == '_' || ch.is_ascii_alphanumeric()) {
			return false;
		}
	}
	true
}

fn allow_only(el: &crate::wui::ast::Element, allowed: &[&str], diags: &mut Vec<Diagnostic>) {
	for attr in &el.attrs {
		if !allowed.contains(&attr.name.as_str()) {
			diags.push(Diagnostic::new(
				format!("unknown attribute {}", attr.name),
				attr.span,
			));
		}
	}
}

fn attr_value_matches(value: &AttrValue, expected: ValueType) -> bool {
	match expected {
		ValueType::String => matches!(attr_value_type(value), ExprType::String | ExprType::Unknown),
		ValueType::Number => matches!(attr_value_type(value), ExprType::Number | ExprType::Unknown),
		ValueType::Bool => matches!(attr_value_type(value), ExprType::Bool | ExprType::Unknown),
		ValueType::Any => !matches!(attr_value_type(value), ExprType::Null),
	}
}

fn attr_value_type(value: &AttrValue) -> ExprType {
	match value {
		AttrValue::String(_, _) => ExprType::String,
		AttrValue::Number(_, _) => ExprType::Number,
		AttrValue::Bool(_, _) => ExprType::Bool,
		AttrValue::Null(_) => ExprType::Null,
		AttrValue::Expr(expr) => expr_type(expr),
	}
}

fn expr_type(expr: &Expr) -> ExprType {
	match expr {
		Expr::Literal(lit, _) => match lit {
			Literal::String(_) => ExprType::String,
			Literal::Number(_) => ExprType::Number,
			Literal::Bool(_) => ExprType::Bool,
			Literal::Null => ExprType::Null,
		},
		Expr::Call { .. } => ExprType::Unknown,
		_ => ExprType::Unknown,
	}
}
