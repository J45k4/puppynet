use crate::wui::ast::{AttrValue, Attribute, Element, Expr, Node};
use crate::wui::diagnostic::{Diagnostic, Span};
use crate::wui::expr::ExprParser;

#[derive(Debug)]
pub struct ParsedFile {
	pub nodes: Vec<Node>,
	pub diagnostics: Vec<Diagnostic>,
}

pub struct Parser<'a> {
	src: &'a str,
	pos: usize,
	diagnostics: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
	pub fn new(src: &'a str) -> Self {
		Self {
			src,
			pos: 0,
			diagnostics: Vec::new(),
		}
	}

	pub fn parse(mut self) -> ParsedFile {
		let nodes = self.parse_nodes(None);
		ParsedFile {
			nodes,
			diagnostics: self.diagnostics,
		}
	}

	fn parse_nodes(&mut self, stop_tag: Option<&str>) -> Vec<Node> {
		let mut nodes = Vec::new();
		loop {
			self.skip_ws();
			if self.eof() {
				break;
			}
			if let Some(tag) = stop_tag {
				if self.peek_str("</") && self.peek_str_at(tag, 2) {
					break;
				}
			}
			if self.peek_char() == Some('<') {
				match self.parse_element() {
					Some(node) => nodes.push(Node::Element(node)),
					None => break,
				}
				continue;
			}
			if self.peek_char() == Some('{') {
				if let Some(expr) = self.parse_expr_node() {
					nodes.push(Node::Expr(expr));
				}
				continue;
			}
			if let Some(text) = self.parse_text() {
				nodes.push(text);
			} else {
				break;
			}
		}
		nodes
	}

	fn parse_element(&mut self) -> Option<Element> {
		let start = self.pos;
		self.expect_char('<')?;
		if self.consume_char('/') {
			self.diagnostics
				.push(Diagnostic::new("unexpected closing tag", self.span_here()));
			return None;
		}
		let name = self.parse_ident()?;
		let attrs = self.parse_attributes();
		self.skip_ws();
		if self.consume_char('/') {
			self.expect_char('>')?;
			let span = Span::new(start, self.pos);
			return Some(Element {
				name,
				attrs,
				children: Vec::new(),
				span,
			});
		}
		self.expect_char('>')?;
		let children = self.parse_nodes(Some(&name));
		self.expect_char('<')?;
		self.expect_char('/')?;
		let closing = self.parse_ident().unwrap_or_default();
		if closing != name {
			let span = Span::new(start, self.pos);
			self.diagnostics
				.push(Diagnostic::new("mismatched closing tag", span));
		}
		self.skip_ws();
		self.expect_char('>')?;
		let span = Span::new(start, self.pos);
		Some(Element {
			name,
			attrs,
			children,
			span,
		})
	}

	fn parse_attributes(&mut self) -> Vec<Attribute> {
		let mut attrs = Vec::new();
		loop {
			self.skip_ws();
			if self.peek_char() == Some('>') || self.peek_str("/>") {
				break;
			}
			let start = self.pos;
			let Some(name) = self.parse_ident() else {
				self.recover_attr();
				continue;
			};
			self.skip_ws();
			let value = if self.consume_char('=') {
				self.skip_ws();
				if self.peek_char() == Some('"') {
					match self.parse_string() {
						Some(value) => {
							let span = Span::new(start, self.pos);
							AttrValue::String(value, span)
						}
						None => AttrValue::String(String::new(), self.span_here()),
					}
				} else if self.peek_char() == Some('{') {
					match self.parse_braced_expr() {
						Some(expr) => AttrValue::Expr(expr),
						None => AttrValue::Expr(Expr::Literal(
							crate::wui::ast::Literal::Null,
							self.span_here(),
						)),
					}
				} else if let Some(lit) = self.parse_bare_literal() {
					lit
				} else {
					self.diagnostics.push(Diagnostic::new(
						"expected attribute value",
						self.span_here(),
					));
					AttrValue::Bool(true, self.span_here())
				}
			} else {
				let span = Span::new(start, self.pos);
				AttrValue::Bool(true, span)
			};
			let span = Span::new(start, self.pos);
			attrs.push(Attribute { name, value, span });
		}
		attrs
	}

	fn parse_bare_literal(&mut self) -> Option<AttrValue> {
		let start = self.pos;
		if self.peek_str("true") && self.is_delim(4) {
			self.pos += 4;
			return Some(AttrValue::Bool(true, Span::new(start, self.pos)));
		}
		if self.peek_str("false") && self.is_delim(5) {
			self.pos += 5;
			return Some(AttrValue::Bool(false, Span::new(start, self.pos)));
		}
		if self.peek_str("null") && self.is_delim(4) {
			self.pos += 4;
			return Some(AttrValue::Null(Span::new(start, self.pos)));
		}
		let mut saw_digit = false;
		let mut saw_dot = false;
		let mut i = self.pos;
		while let Some(ch) = self.peek_char_at(i) {
			if ch.is_ascii_digit() {
				saw_digit = true;
				i += ch.len_utf8();
				continue;
			}
			if ch == '.' && !saw_dot {
				saw_dot = true;
				i += ch.len_utf8();
				continue;
			}
			break;
		}
		if saw_digit {
			let slice = &self.src[self.pos..i];
			if let Ok(value) = slice.parse::<f64>() {
				self.pos = i;
				return Some(AttrValue::Number(value, Span::new(start, self.pos)));
			}
		}
		None
	}

	fn parse_text(&mut self) -> Option<Node> {
		let start = self.pos;
		let mut end = self.pos;
		while let Some(ch) = self.peek_char() {
			if ch == '<' || ch == '{' {
				break;
			}
			self.pos += ch.len_utf8();
			end = self.pos;
		}
		if end == start {
			return None;
		}
		let text = self.src[start..end].to_string();
		if text.trim().is_empty() {
			return None;
		}
		Some(Node::Text(text, Span::new(start, end)))
	}

	fn parse_expr_node(&mut self) -> Option<Expr> {
		self.parse_braced_expr()
	}

	fn parse_braced_expr(&mut self) -> Option<Expr> {
		let start = self.pos;
		if !self.consume_char('{') {
			return None;
		}
		let expr_start = self.pos;
		let mut in_string = false;
		while let Some(ch) = self.peek_char() {
			if ch == '"' {
				in_string = !in_string;
			}
			if ch == '}' && !in_string {
				break;
			}
			self.pos += ch.len_utf8();
		}
		if self.peek_char() != Some('}') {
			self.diagnostics
				.push(Diagnostic::new("unterminated expression", self.span_here()));
			return None;
		}
		let expr_end = self.pos;
		let expr_src = &self.src[expr_start..expr_end];
		self.pos += 1;
		match ExprParser::new(expr_src, expr_start).parse() {
			Ok(expr) => Some(expr),
			Err(diag) => {
				self.diagnostics.push(diag);
				self.recover_to('}');
				Some(Expr::Literal(
					crate::wui::ast::Literal::Null,
					Span::new(start, self.pos),
				))
			}
		}
	}

	fn parse_string(&mut self) -> Option<String> {
		if !self.consume_char('"') {
			return None;
		}
		let mut out = String::new();
		while let Some(ch) = self.peek_char() {
			self.pos += ch.len_utf8();
			match ch {
				'"' => return Some(out),
				'\\' => {
					let next = self.peek_char()?;
					self.pos += next.len_utf8();
					match next {
						'"' => out.push('"'),
						'\\' => out.push('\\'),
						'n' => out.push('\n'),
						_ => {
							self.diagnostics
								.push(Diagnostic::new("invalid escape", self.span_here()));
						}
					}
				}
				_ => out.push(ch),
			}
		}
		self.diagnostics
			.push(Diagnostic::new("unterminated string", self.span_here()));
		None
	}

	fn parse_ident(&mut self) -> Option<String> {
		self.skip_ws();
		let Some(first) = self.peek_char() else {
			return None;
		};
		if !(first.is_ascii_alphabetic() || first == '_' || first == ':') {
			return None;
		}
		let mut end = self.pos + first.len_utf8();
		let mut iter = self.src[self.pos + first.len_utf8()..].char_indices();
		while let Some((offset, ch)) = iter.next() {
			if ch.is_ascii_alphanumeric() || ch == '_' || ch == ':' || ch == '-' {
				end = self.pos + first.len_utf8() + offset + ch.len_utf8();
			} else {
				break;
			}
		}
		let ident = self.src[self.pos..end].to_string();
		self.pos = end;
		Some(ident)
	}

	fn recover_attr(&mut self) {
		while let Some(ch) = self.peek_char() {
			if ch.is_whitespace() || ch == '>' || ch == '/' {
				break;
			}
			self.pos += ch.len_utf8();
		}
	}

	fn recover_to(&mut self, target: char) {
		while let Some(ch) = self.peek_char() {
			self.pos += ch.len_utf8();
			if ch == target {
				break;
			}
		}
	}

	fn skip_ws(&mut self) {
		while let Some(ch) = self.peek_char() {
			if ch.is_whitespace() {
				self.pos += ch.len_utf8();
			} else {
				break;
			}
		}
	}

	fn expect_char(&mut self, ch: char) -> Option<()> {
		self.skip_ws();
		if self.consume_char(ch) {
			Some(())
		} else {
			self.diagnostics
				.push(Diagnostic::new("expected character", self.span_here()));
			None
		}
	}

	fn consume_char(&mut self, ch: char) -> bool {
		self.skip_ws();
		if self.peek_char() == Some(ch) {
			self.pos += ch.len_utf8();
			true
		} else {
			false
		}
	}

	fn peek_char(&self) -> Option<char> {
		self.src[self.pos..].chars().next()
	}

	fn peek_char_at(&self, pos: usize) -> Option<char> {
		self.src[pos..].chars().next()
	}

	fn peek_str(&self, s: &str) -> bool {
		self.src[self.pos..].starts_with(s)
	}

	fn peek_str_at(&self, s: &str, offset: usize) -> bool {
		self.src[self.pos + offset..].starts_with(s)
	}

	fn is_delim(&self, len: usize) -> bool {
		let idx = self.pos + len;
		if idx >= self.src.len() {
			return true;
		}
		match self.src[idx..].chars().next() {
			Some(ch) => !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '.'),
			None => true,
		}
	}

	fn span_here(&self) -> Span {
		Span::new(self.pos, self.pos)
	}

	fn eof(&self) -> bool {
		self.pos >= self.src.len()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_simple_element() {
		let file = Parser::new("<Text value=\"hi\" />").parse();
		assert!(file.diagnostics.is_empty());
		assert_eq!(file.nodes.len(), 1);
	}

	#[test]
	fn parses_attributes_and_children() {
		let file = Parser::new("<VStack spacing=4><Text value={state.title} /></VStack>").parse();
		assert!(file.diagnostics.is_empty());
		assert_eq!(file.nodes.len(), 1);
		let Node::Element(root) = &file.nodes[0] else {
			panic!("expected element");
		};
		assert_eq!(root.name, "VStack");
		assert_eq!(root.attrs.len(), 1);
		match &root.attrs[0].value {
			AttrValue::Number(value, _) => assert_eq!(*value, 4.0),
			_ => panic!("expected number attribute"),
		}
		assert_eq!(root.children.len(), 1);
		let Node::Element(child) = &root.children[0] else {
			panic!("expected child element");
		};
		assert_eq!(child.name, "Text");
		match &child.attrs[0].value {
			AttrValue::Expr(Expr::Path(parts, _)) => {
				assert_eq!(parts, &vec!["state".to_string(), "title".to_string()]);
			}
			_ => panic!("expected path expression"),
		}
	}

	#[test]
	fn reports_mismatched_closing_tag() {
		let file = Parser::new("<Text></Button>").parse();
		assert!(!file.diagnostics.is_empty());
	}
}
