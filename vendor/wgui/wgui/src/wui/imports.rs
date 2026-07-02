use crate::wui::ast::{AttrValue, Element, Node};
use crate::wui::diagnostic::{Diagnostic, Span};
use crate::wui::parser::Parser;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct ImportResult {
	pub nodes: Vec<Node>,
	pub components: HashMap<String, Vec<Node>>,
	pub source_files: Vec<PathBuf>,
}

#[derive(Clone)]
struct ImportedTemplate {
	nodes: Vec<Node>,
	components: HashMap<String, Vec<Node>>,
}

#[derive(Default)]
struct ImportContext {
	cache: HashMap<PathBuf, ImportedTemplate>,
	stack: Vec<PathBuf>,
	source_files: Vec<PathBuf>,
	source_files_set: HashSet<PathBuf>,
}

impl ImportContext {
	fn record_source(&mut self, path: &PathBuf) {
		if self.source_files_set.insert(path.clone()) {
			self.source_files.push(path.clone());
		}
	}
}

pub fn resolve(
	source: &str,
	module_name: &str,
	base_dir: Option<&Path>,
) -> Result<ImportResult, Vec<Diagnostic>> {
	let parsed = Parser::new(source).parse();
	let mut diags = parsed.diagnostics;
	let mut ctx = ImportContext::default();
	let mut components = HashMap::new();
	let nodes = expand_nodes(
		&parsed.nodes,
		base_dir,
		module_name,
		&mut ctx,
		&mut components,
		&mut diags,
	);
	if diags.is_empty() {
		Ok(ImportResult {
			nodes,
			components,
			source_files: ctx.source_files,
		})
	} else {
		Err(diags)
	}
}

fn expand_nodes(
	nodes: &[Node],
	base_dir: Option<&Path>,
	module_name: &str,
	ctx: &mut ImportContext,
	components: &mut HashMap<String, Vec<Node>>,
	diags: &mut Vec<Diagnostic>,
) -> Vec<Node> {
	let mut out = Vec::new();
	for node in nodes {
		match node {
			Node::Element(el) if el.name == "Import" => {
				if !el.children.is_empty() {
					diags.push(Diagnostic::new("Import tags cannot have children", el.span));
				}
				if let Some(name) = import_name(el, diags) {
					let Some(src) = import_component_src(el, diags) else {
						continue;
					};
					let Some(path) =
						resolve_import_path(&src, base_dir, module_name, el.span, diags)
					else {
						continue;
					};
					if let Some(imported) = load_import(&path, module_name, el.span, ctx, diags) {
						merge_components(components, imported.components, el.span, diags);
						if components.insert(name.clone(), imported.nodes).is_some() {
							diags.push(Diagnostic::new(
								format!("duplicate import component {}", name),
								el.span,
							));
						}
					}
				} else {
					let Some(src) = import_src(el, diags) else {
						continue;
					};
					let Some(path) =
						resolve_import_path(&src, base_dir, module_name, el.span, diags)
					else {
						continue;
					};
					if let Some(imported) = load_import(&path, module_name, el.span, ctx, diags) {
						merge_components(components, imported.components, el.span, diags);
						out.extend(imported.nodes);
					}
				}
			}
			Node::Element(el) => {
				let mut updated = el.clone();
				updated.children =
					expand_nodes(&el.children, base_dir, module_name, ctx, components, diags);
				out.push(Node::Element(updated));
			}
			_ => out.push(node.clone()),
		}
	}
	out
}

fn merge_components(
	components: &mut HashMap<String, Vec<Node>>,
	imported: HashMap<String, Vec<Node>>,
	span: Span,
	diags: &mut Vec<Diagnostic>,
) {
	for (name, nodes) in imported {
		if components.insert(name.clone(), nodes).is_some() {
			diags.push(Diagnostic::new(
				format!("duplicate import component {}", name),
				span,
			));
		}
	}
}

fn import_name(el: &Element, diags: &mut Vec<Diagnostic>) -> Option<String> {
	let Some(attr) = el.attrs.iter().find(|attr| attr.name == "name") else {
		return None;
	};
	match &attr.value {
		AttrValue::String(value, _) if !value.trim().is_empty() => Some(value.clone()),
		AttrValue::String(_, _) => {
			diags.push(Diagnostic::new("Import name cannot be empty", attr.span));
			None
		}
		_ => {
			diags.push(Diagnostic::new(
				"Import name must be a string literal",
				attr.span,
			));
			None
		}
	}
}

fn import_src(el: &Element, diags: &mut Vec<Diagnostic>) -> Option<String> {
	let Some(attr) = el.attrs.iter().find(|attr| attr.name == "src") else {
		diags.push(Diagnostic::new(
			"Import requires src=\"...\" or name=\"...\" from=\"...\"",
			el.span,
		));
		return None;
	};
	match &attr.value {
		AttrValue::String(value, _) => Some(value.clone()),
		_ => {
			diags.push(Diagnostic::new(
				"Import src must be a string literal",
				attr.span,
			));
			None
		}
	}
}

fn import_component_src(el: &Element, diags: &mut Vec<Diagnostic>) -> Option<String> {
	let attr = el
		.attrs
		.iter()
		.find(|attr| attr.name == "from")
		.or_else(|| el.attrs.iter().find(|attr| attr.name == "src"));
	let Some(attr) = attr else {
		diags.push(Diagnostic::new(
			"component Import requires from=\"...\"",
			el.span,
		));
		return None;
	};
	match &attr.value {
		AttrValue::String(value, _) => Some(value.clone()),
		_ => {
			diags.push(Diagnostic::new(
				"Import from must be a string literal",
				attr.span,
			));
			None
		}
	}
}

fn resolve_import_path(
	src: &str,
	base_dir: Option<&Path>,
	module_name: &str,
	span: Span,
	diags: &mut Vec<Diagnostic>,
) -> Option<PathBuf> {
	let path = PathBuf::from(src);
	if path.is_absolute() {
		return Some(path);
	}
	let Some(base_dir) = base_dir else {
		diags.push(Diagnostic::new(
			format!(
				"cannot resolve import without a base directory (module {})",
				module_name
			),
			span,
		));
		return None;
	};
	let joined = base_dir.join(&path);
	if joined.exists() || joined.extension().is_some() {
		return Some(joined);
	}
	Some(joined.with_extension("wui"))
}

fn load_import(
	path: &PathBuf,
	module_name: &str,
	span: Span,
	ctx: &mut ImportContext,
	diags: &mut Vec<Diagnostic>,
) -> Option<ImportedTemplate> {
	let normalized = normalize_path(path);
	if ctx.stack.contains(&normalized) {
		diags.push(Diagnostic::new(
			format!("circular import detected at {}", path.display()),
			span,
		));
		return None;
	}
	if let Some(cached) = ctx.cache.get(&normalized) {
		return Some(cached.clone());
	}
	let source = match fs::read_to_string(path) {
		Ok(source) => source,
		Err(err) => {
			diags.push(Diagnostic::new(
				format!("failed to read import {}: {}", path.display(), err),
				span,
			));
			return None;
		}
	};
	ctx.record_source(path);
	ctx.stack.push(normalized.clone());
	let parsed = Parser::new(&source).parse();
	diags.extend(parsed.diagnostics);
	let mut components = HashMap::new();
	let nodes = expand_nodes(
		&parsed.nodes,
		path.parent(),
		module_name,
		ctx,
		&mut components,
		diags,
	);
	ctx.stack.pop();
	let imported = ImportedTemplate { nodes, components };
	ctx.cache.insert(normalized, imported.clone());
	Some(imported)
}

fn normalize_path(path: &Path) -> PathBuf {
	fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::fs;
	use std::time::{SystemTime, UNIX_EPOCH};

	#[test]
	fn resolves_imports_from_disk() {
		let suffix = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap()
			.as_nanos();
		let dir = std::env::temp_dir().join(format!("wui_import_test_{}", suffix));
		fs::create_dir_all(&dir).expect("create temp dir");
		let partial = dir.join("partial.wui");
		fs::write(&partial, "<Text value=\"hi\" />").expect("write partial");
		let src = "<Import src=\"partial.wui\" />";
		let result = resolve(src, "main", Some(&dir)).expect("resolve imports");
		assert_eq!(result.nodes.len(), 1);
		assert!(result.components.is_empty());
		assert_eq!(result.source_files.len(), 1);
		assert_eq!(result.source_files[0], partial);
	}

	#[test]
	fn resolves_named_imports_as_components() {
		let suffix = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap()
			.as_nanos();
		let dir = std::env::temp_dir().join(format!("wui_component_import_test_{}", suffix));
		fs::create_dir_all(&dir).expect("create temp dir");
		let layout = dir.join("layout.wui");
		fs::write(&layout, "<VStack><Children /></VStack>").expect("write layout");
		let src = r#"<Import name="AppLayout" from="layout" /><AppLayout />"#;
		let result = resolve(src, "main", Some(&dir)).expect("resolve imports");
		assert_eq!(result.nodes.len(), 1);
		assert!(result.components.contains_key("AppLayout"));
		assert_eq!(result.source_files, vec![layout]);
	}
}
