use crate::wui::ast::{BinaryOp, Expr, Literal, UnaryOp};
use crate::wui::diagnostic::{Diagnostic, Span};

#[derive(Debug, Clone)]
pub struct ExprParser<'a> {
	src: &'a str,
	offset: usize,
	pos: usize,
}

impl<'a> ExprParser<'a> {
	pub fn new(src: &'a str, offset: usize) -> Self {
		Self {
			src,
			offset,
			pos: 0,
		}
	}

	pub fn parse(mut self) -> Result<Expr, Diagnostic> {
		let expr = self.parse_ternary()?;
		self.skip_ws();
		if self.pos < self.src.len() {
			let span = self.span_here();
			return Err(Diagnostic::new("unexpected token", span));
		}
		Ok(expr)
	}

	fn parse_ternary(&mut self) -> Result<Expr, Diagnostic> {
		let start = self.current_offset();
		let mut expr = self.parse_coalesce()?;
		self.skip_ws();
		if self.consume_char('?') {
			let then_expr = self.parse_ternary()?;
			self.expect_char(':')?;
			let else_expr = self.parse_ternary()?;
			let span = Span::new(start, else_expr.span().end);
			expr = Expr::Ternary {
				cond: Box::new(expr),
				then_expr: Box::new(then_expr),
				else_expr: Box::new(else_expr),
				span,
			};
		}
		Ok(expr)
	}

	fn parse_coalesce(&mut self) -> Result<Expr, Diagnostic> {
		let start = self.current_offset();
		let mut expr = self.parse_or()?;
		self.skip_ws();
		while self.peek_str("??") {
			self.pos += 2;
			let right = self.parse_or()?;
			let span = Span::new(start, right.span().end);
			expr = Expr::Coalesce {
				left: Box::new(expr),
				right: Box::new(right),
				span,
			};
			self.skip_ws();
		}
		Ok(expr)
	}

	fn parse_or(&mut self) -> Result<Expr, Diagnostic> {
		self.parse_binary(Self::parse_and, &[("||", BinaryOp::Or)])
	}

	fn parse_and(&mut self) -> Result<Expr, Diagnostic> {
		self.parse_binary(Self::parse_equality, &[("&&", BinaryOp::And)])
	}

	fn parse_equality(&mut self) -> Result<Expr, Diagnostic> {
		self.parse_binary(
			Self::parse_compare,
			&[("==", BinaryOp::Eq), ("!=", BinaryOp::Neq)],
		)
	}

	fn parse_compare(&mut self) -> Result<Expr, Diagnostic> {
		self.parse_binary(
			Self::parse_add,
			&[
				("<=", BinaryOp::Lte),
				(">=", BinaryOp::Gte),
				("<", BinaryOp::Lt),
				(">", BinaryOp::Gt),
			],
		)
	}

	fn parse_add(&mut self) -> Result<Expr, Diagnostic> {
		self.parse_binary(
			Self::parse_mul,
			&[("+", BinaryOp::Add), ("-", BinaryOp::Sub)],
		)
	}

	fn parse_mul(&mut self) -> Result<Expr, Diagnostic> {
		self.parse_binary(
			Self::parse_unary,
			&[
				("*", BinaryOp::Mul),
				("/", BinaryOp::Div),
				("%", BinaryOp::Mod),
			],
		)
	}

	fn parse_binary<F>(
		&mut self,
		next: F,
		ops: &[(&'static str, BinaryOp)],
	) -> Result<Expr, Diagnostic>
	where
		F: Fn(&mut Self) -> Result<Expr, Diagnostic>,
	{
		let mut expr = next(self)?;
		loop {
			self.skip_ws();
			let mut matched = None;
			for (token, op) in ops {
				if self.peek_str(token) {
					matched = Some((*token, *op));
					break;
				}
			}
			let Some((token, op)) = matched else {
				break;
			};
			self.pos += token.len();
			let right = next(self)?;
			let span = Span::new(expr.span().start, right.span().end);
			expr = Expr::Binary {
				left: Box::new(expr),
				op,
				right: Box::new(right),
				span,
			};
		}
		Ok(expr)
	}

	fn parse_unary(&mut self) -> Result<Expr, Diagnostic> {
		self.skip_ws();
		let start = self.current_offset();
		if self.consume_char('!') {
			let expr = self.parse_unary()?;
			let span = Span::new(start, expr.span().end);
			return Ok(Expr::Unary {
				op: UnaryOp::Not,
				expr: Box::new(expr),
				span,
			});
		}
		if self.consume_char('-') {
			let expr = self.parse_unary()?;
			let span = Span::new(start, expr.span().end);
			return Ok(Expr::Unary {
				op: UnaryOp::Neg,
				expr: Box::new(expr),
				span,
			});
		}
		self.parse_primary()
	}

	fn parse_primary(&mut self) -> Result<Expr, Diagnostic> {
		self.skip_ws();
		let start = self.current_offset();
		if self.consume_char('(') {
			let expr = self.parse_ternary()?;
			self.expect_char(')')?;
			return Ok(expr);
		}
		if let Some(value) = self.parse_string()? {
			let span = Span::new(start, self.current_offset());
			return Ok(Expr::Literal(Literal::String(value), span));
		}
		if let Some(lit) = self.parse_literal()? {
			let span = Span::new(start, self.current_offset());
			return Ok(Expr::Literal(lit, span));
		}
		if let Some(path) = self.parse_path()? {
			self.skip_ws();
			if self.consume_char('(') {
				let span_start = start;
				let Some(name) = single_path_name(&path) else {
					return Err(Diagnostic::new("expected function name", self.span_here()));
				};
				let args = self.parse_call_args()?;
				let span = Span::new(span_start, self.current_offset());
				return Ok(Expr::Call { name, args, span });
			}
			let span = Span::new(start, self.current_offset());
			return Ok(Expr::Path(path, span));
		}
		Err(Diagnostic::new("expected expression", self.span_here()))
	}

	fn parse_call_args(&mut self) -> Result<Vec<Expr>, Diagnostic> {
		let mut args = Vec::new();
		self.skip_ws();
		if self.consume_char(')') {
			return Ok(args);
		}
		loop {
			args.push(self.parse_ternary()?);
			self.skip_ws();
			if self.consume_char(')') {
				return Ok(args);
			}
			self.expect_char(',')?;
		}
	}

	fn parse_string(&mut self) -> Result<Option<String>, Diagnostic> {
		self.skip_ws();
		if !self.consume_char('"') {
			return Ok(None);
		}
		let mut out = String::new();
		while let Some(ch) = self.peek_char() {
			self.pos += ch.len_utf8();
			match ch {
				'"' => return Ok(Some(out)),
				'\\' => {
					let next = self
						.peek_char()
						.ok_or_else(|| Diagnostic::new("unterminated escape", self.span_here()))?;
					self.pos += next.len_utf8();
					match next {
						'"' => out.push('"'),
						'\\' => out.push('\\'),
						'n' => out.push('\n'),
						_ => return Err(Diagnostic::new("invalid escape", self.span_here())),
					}
				}
				_ => out.push(ch),
			}
		}
		Err(Diagnostic::new("unterminated string", self.span_here()))
	}

	fn parse_literal(&mut self) -> Result<Option<Literal>, Diagnostic> {
		self.skip_ws();
		if self.peek_str("true") && self.is_delim(4) {
			self.pos += 4;
			return Ok(Some(Literal::Bool(true)));
		}
		if self.peek_str("false") && self.is_delim(5) {
			self.pos += 5;
			return Ok(Some(Literal::Bool(false)));
		}
		if self.peek_str("null") && self.is_delim(4) {
			self.pos += 4;
			return Ok(Some(Literal::Null));
		}
		let start = self.pos;
		let mut saw_digit = false;
		let mut saw_dot = false;
		while let Some(ch) = self.peek_char() {
			if ch.is_ascii_digit() {
				saw_digit = true;
				self.pos += 1;
				continue;
			}
			if ch == '.' && !saw_dot {
				saw_dot = true;
				self.pos += 1;
				continue;
			}
			break;
		}
		if saw_digit {
			let slice = &self.src[start..self.pos];
			let value = slice
				.parse::<f64>()
				.map_err(|_| Diagnostic::new("invalid number", self.span_here()))?;
			return Ok(Some(Literal::Number(value)));
		}
		Ok(None)
	}

	fn parse_path(&mut self) -> Result<Option<Vec<String>>, Diagnostic> {
		self.skip_ws();
		let Some(ident) = self.parse_ident() else {
			return Ok(None);
		};
		let mut parts = vec![ident];
		loop {
			self.skip_ws();
			if !self.consume_char('.') {
				break;
			}
			let ident = self
				.parse_ident()
				.ok_or_else(|| Diagnostic::new("expected identifier", self.span_here()))?;
			parts.push(ident);
		}
		Ok(Some(parts))
	}

	fn parse_ident(&mut self) -> Option<String> {
		self.skip_ws();
		let mut iter = self.src[self.pos..].char_indices();
		let Some((_, first)) = iter.next() else {
			return None;
		};
		if !(first.is_ascii_alphabetic() || first == '_') {
			return None;
		}
		let mut end = self.pos + first.len_utf8();
		for (offset, ch) in iter {
			if ch.is_ascii_alphanumeric() || ch == '_' {
				end = self.pos + offset + ch.len_utf8();
			} else {
				break;
			}
		}
		let ident = self.src[self.pos..end].to_string();
		self.pos = end;
		Some(ident)
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

	fn consume_char(&mut self, ch: char) -> bool {
		self.skip_ws();
		if self.peek_char() == Some(ch) {
			self.pos += ch.len_utf8();
			true
		} else {
			false
		}
	}

	fn expect_char(&mut self, ch: char) -> Result<(), Diagnostic> {
		if self.consume_char(ch) {
			Ok(())
		} else {
			Err(Diagnostic::new("expected character", self.span_here()))
		}
	}

	fn peek_char(&self) -> Option<char> {
		self.src[self.pos..].chars().next()
	}

	fn peek_str(&self, s: &str) -> bool {
		self.src[self.pos..].starts_with(s)
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
		let pos = self.current_offset();
		Span::new(pos, pos)
	}

	fn current_offset(&self) -> usize {
		self.offset + self.pos
	}
}

fn single_path_name(path: &[String]) -> Option<String> {
	if path.len() == 1 {
		Some(path[0].clone())
	} else {
		None
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parse_simple_expression() {
		let expr = ExprParser::new("state.count + 1", 0).parse().unwrap();
		match expr {
			Expr::Binary { op, .. } => assert!(matches!(op, BinaryOp::Add)),
			_ => panic!("expected binary"),
		}
	}

	#[test]
	fn parse_call_expression() {
		let expr = ExprParser::new("path_matches(activeWhen ?? href)", 0)
			.parse()
			.unwrap();
		match expr {
			Expr::Call { name, args, .. } => {
				assert_eq!(name, "path_matches");
				assert_eq!(args.len(), 1);
			}
			_ => panic!("expected call"),
		}
	}
}
