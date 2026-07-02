use crate::wui::diagnostic::Span;

#[derive(Debug, Clone)]
pub enum Node {
	Element(Element),
	Text(String, Span),
	Expr(Expr),
}

#[derive(Debug, Clone)]
pub struct Element {
	pub name: String,
	pub attrs: Vec<Attribute>,
	pub children: Vec<Node>,
	pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Attribute {
	pub name: String,
	pub value: AttrValue,
	pub span: Span,
}

#[derive(Debug, Clone)]
pub enum AttrValue {
	String(String, Span),
	Number(f64, Span),
	Bool(bool, Span),
	Null(Span),
	Expr(Expr),
}

#[derive(Debug, Clone)]
pub enum Expr {
	Literal(Literal, Span),
	Path(Vec<String>, Span),
	Call {
		name: String,
		args: Vec<Expr>,
		span: Span,
	},
	Unary {
		op: UnaryOp,
		expr: Box<Expr>,
		span: Span,
	},
	Binary {
		left: Box<Expr>,
		op: BinaryOp,
		right: Box<Expr>,
		span: Span,
	},
	Ternary {
		cond: Box<Expr>,
		then_expr: Box<Expr>,
		else_expr: Box<Expr>,
		span: Span,
	},
	Coalesce {
		left: Box<Expr>,
		right: Box<Expr>,
		span: Span,
	},
}

#[derive(Debug, Clone)]
pub enum Literal {
	String(String),
	Number(f64),
	Bool(bool),
	Null,
}

#[derive(Debug, Clone, Copy)]
pub enum UnaryOp {
	Not,
	Neg,
}

#[derive(Debug, Clone, Copy)]
pub enum BinaryOp {
	Add,
	Sub,
	Mul,
	Div,
	Mod,
	Eq,
	Neq,
	Lt,
	Lte,
	Gt,
	Gte,
	And,
	Or,
}

impl Expr {
	pub fn span(&self) -> Span {
		match self {
			Expr::Literal(_, span)
			| Expr::Path(_, span)
			| Expr::Call { span, .. }
			| Expr::Unary { span, .. }
			| Expr::Binary { span, .. }
			| Expr::Ternary { span, .. }
			| Expr::Coalesce { span, .. } => *span,
		}
	}
}
