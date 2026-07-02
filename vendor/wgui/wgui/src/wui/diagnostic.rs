#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
	pub start: usize,
	pub end: usize,
}

impl Span {
	pub fn new(start: usize, end: usize) -> Self {
		Self { start, end }
	}
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
	pub message: String,
	pub span: Span,
}

impl Diagnostic {
	pub fn new(message: impl Into<String>, span: Span) -> Self {
		Self {
			message: message.into(),
			span,
		}
	}
}
