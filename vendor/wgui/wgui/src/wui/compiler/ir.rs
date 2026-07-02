use crate::wui::ast::Expr;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct IrDocument {
	pub nodes: Vec<IrNode>,
	pub components: HashMap<String, IrComponent>,
	pub actions: Vec<ActionDef>,
	pub pages: Vec<PageMeta>,
}

#[derive(Debug, Clone)]
pub struct IrComponent {
	pub body: Vec<IrNode>,
}

#[derive(Debug, Clone)]
pub struct PageMeta {
	pub module: String,
	pub route: Option<String>,
	pub title: Option<String>,
	pub state_type: Option<String>,
}

#[derive(Debug, Clone)]
pub enum IrNode {
	Widget(IrWidget),
	For(IrFor),
	If(IrIf),
	Scope(IrScope),
	Route(IrRoute),
	Switch(IrSwitch),
	Children,
	Text(String),
}

#[derive(Debug, Clone)]
pub struct IrWidget {
	pub tag: String,
	pub props: Vec<IrProp>,
	pub children: Vec<IrNode>,
}

#[derive(Debug, Clone)]
pub struct IrFor {
	pub each: Expr,
	pub item: String,
	pub index: Option<String>,
	pub key: Option<Expr>,
	pub body: Vec<IrNode>,
}

#[derive(Debug, Clone)]
pub struct IrIf {
	pub test: Expr,
	pub then_body: Vec<IrNode>,
	pub else_body: Vec<IrNode>,
}

#[derive(Debug, Clone)]
pub struct IrScope {
	pub name: String,
	pub body: Vec<IrNode>,
}

#[derive(Debug, Clone)]
pub struct IrRoute {
	pub path: String,
	pub body: Vec<IrNode>,
}

#[derive(Debug, Clone)]
pub struct IrSwitch {
	pub cases: Vec<IrRoute>,
}

#[derive(Debug, Clone)]
pub enum IrProp {
	Value {
		name: String,
		expr: Expr,
	},
	Literal {
		name: String,
		value: String,
	},
	Bool {
		name: String,
		value: bool,
	},
	Number {
		name: String,
		value: f64,
	},
	Event {
		name: String,
		action: String,
		arg: Option<Expr>,
	},
	Bind {
		name: String,
		expr: Expr,
	},
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
	Click,
	TextChanged,
	SliderChange,
	Select,
	Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionPayload {
	None,
	U32,
	String,
	I32,
	U32I32,
	Json,
}

#[derive(Debug, Clone)]
pub struct ActionDef {
	pub name: String,
	pub kind: EventKind,
	pub payload: ActionPayload,
	pub id: u32,
}
