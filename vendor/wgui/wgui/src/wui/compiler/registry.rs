use crate::wui::compiler::ir::EventKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
	String,
	Number,
	Bool,
	Any,
}

#[derive(Debug, Clone)]
pub enum PropKind {
	Value(ValueType),
	Event(EventKind),
	Bind(ValueType),
}

#[derive(Debug, Clone)]
pub struct PropSchema {
	pub name: &'static str,
	pub kind: PropKind,
}

#[derive(Debug, Clone)]
pub struct TagSchema {
	pub name: &'static str,
	pub props: &'static [PropSchema],
}

pub fn schema_for(tag: &str) -> Option<TagSchema> {
	match tag {
		"VStack" => Some(TagSchema {
			name: "VStack",
			props: layout_props(),
		}),
		"HStack" => Some(TagSchema {
			name: "HStack",
			props: layout_props(),
		}),
		"Text" => Some(TagSchema {
			name: "Text",
			props: &[
				PropSchema {
					name: "value",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "grow",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "minWidth",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "breakWords",
					kind: PropKind::Value(ValueType::Bool),
				},
				PropSchema {
					name: "fill",
					kind: PropKind::Value(ValueType::Bool),
				},
				PropSchema {
					name: "textAlign",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "color",
					kind: PropKind::Value(ValueType::String),
				},
			],
		}),
		"Button" => Some(TagSchema {
			name: "Button",
			props: &[
				PropSchema {
					name: "text",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "onClick",
					kind: PropKind::Event(EventKind::Click),
				},
				PropSchema {
					name: "arg",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "backgroundColor",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "border",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "color",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "width",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "height",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "padding",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "paddingLeft",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "paddingRight",
					kind: PropKind::Value(ValueType::Number),
				},
			],
		}),
		"Link" => Some(TagSchema {
			name: "Link",
			props: &[
				PropSchema {
					name: "href",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "text",
					kind: PropKind::Value(ValueType::String),
				},
			],
		}),
		"TextInput" => Some(TagSchema {
			name: "TextInput",
			props: &[
				PropSchema {
					name: "value",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "bind:value",
					kind: PropKind::Bind(ValueType::String),
				},
				PropSchema {
					name: "placeholder",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "onTextChanged",
					kind: PropKind::Event(EventKind::TextChanged),
				},
				PropSchema {
					name: "grow",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "minWidth",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "fill",
					kind: PropKind::Value(ValueType::Bool),
				},
				PropSchema {
					name: "backgroundColor",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "border",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "color",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "height",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "padding",
					kind: PropKind::Value(ValueType::Number),
				},
			],
		}),
		"DatePicker" => Some(TagSchema {
			name: "DatePicker",
			props: &[
				PropSchema {
					name: "value",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "bind:value",
					kind: PropKind::Bind(ValueType::String),
				},
				PropSchema {
					name: "placeholder",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "onTextChanged",
					kind: PropKind::Event(EventKind::TextChanged),
				},
				PropSchema {
					name: "grow",
					kind: PropKind::Value(ValueType::Number),
				},
			],
		}),
		"Checkbox" => Some(TagSchema {
			name: "Checkbox",
			props: &[
				PropSchema {
					name: "checked",
					kind: PropKind::Value(ValueType::Bool),
				},
				PropSchema {
					name: "bind:checked",
					kind: PropKind::Bind(ValueType::Bool),
				},
				PropSchema {
					name: "onClick",
					kind: PropKind::Event(EventKind::Click),
				},
				PropSchema {
					name: "arg",
					kind: PropKind::Value(ValueType::Number),
				},
			],
		}),
		"Slider" => Some(TagSchema {
			name: "Slider",
			props: &[
				PropSchema {
					name: "min",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "max",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "value",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "step",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "onSliderChange",
					kind: PropKind::Event(EventKind::SliderChange),
				},
				PropSchema {
					name: "arg",
					kind: PropKind::Value(ValueType::Number),
				},
			],
		}),
		"Custom" | "CustomComponent" => Some(TagSchema {
			name: "CustomComponent",
			props: &[
				PropSchema {
					name: "name",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "entry",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "props",
					kind: PropKind::Value(ValueType::Any),
				},
				PropSchema {
					name: "width",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "height",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "minWidth",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "maxWidth",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "minHeight",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "maxHeight",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "grow",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "fill",
					kind: PropKind::Value(ValueType::Bool),
				},
				PropSchema {
					name: "backgroundColor",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "border",
					kind: PropKind::Value(ValueType::String),
				},
			],
		}),
		"Image" => Some(TagSchema {
			name: "Image",
			props: &[
				PropSchema {
					name: "src",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "alt",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "objectFit",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "maxWidth",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "maxHeight",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "onClick",
					kind: PropKind::Event(EventKind::Click),
				},
				PropSchema {
					name: "arg",
					kind: PropKind::Value(ValueType::Number),
				},
			],
		}),
		"Video" => Some(TagSchema {
			name: "Video",
			props: &[
				PropSchema {
					name: "room",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "local",
					kind: PropKind::Value(ValueType::Bool),
				},
				PropSchema {
					name: "autoplay",
					kind: PropKind::Value(ValueType::Bool),
				},
				PropSchema {
					name: "muted",
					kind: PropKind::Value(ValueType::Bool),
				},
				PropSchema {
					name: "controls",
					kind: PropKind::Value(ValueType::Bool),
				},
				PropSchema {
					name: "maxWidth",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "maxHeight",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "minWidth",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "minHeight",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "width",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "height",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "onClick",
					kind: PropKind::Event(EventKind::Click),
				},
				PropSchema {
					name: "arg",
					kind: PropKind::Value(ValueType::Number),
				},
			],
		}),
		"Audio" => Some(TagSchema {
			name: "Audio",
			props: &[
				PropSchema {
					name: "room",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "local",
					kind: PropKind::Value(ValueType::Bool),
				},
				PropSchema {
					name: "autoplay",
					kind: PropKind::Value(ValueType::Bool),
				},
				PropSchema {
					name: "muted",
					kind: PropKind::Value(ValueType::Bool),
				},
				PropSchema {
					name: "controls",
					kind: PropKind::Value(ValueType::Bool),
				},
				PropSchema {
					name: "onClick",
					kind: PropKind::Event(EventKind::Click),
				},
				PropSchema {
					name: "arg",
					kind: PropKind::Value(ValueType::Number),
				},
			],
		}),
		"FolderPicker" => Some(TagSchema {
			name: "FolderPicker",
			props: &[PropSchema {
				name: "onTextChanged",
				kind: PropKind::Event(EventKind::TextChanged),
			}],
		}),
		"Modal" => Some(TagSchema {
			name: "Modal",
			props: &[
				PropSchema {
					name: "open",
					kind: PropKind::Value(ValueType::Bool),
				},
				PropSchema {
					name: "onClick",
					kind: PropKind::Event(EventKind::Click),
				},
				PropSchema {
					name: "padding",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "paddingLeft",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "paddingRight",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "paddingTop",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "paddingBottom",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "margin",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "marginLeft",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "marginRight",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "marginTop",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "marginBottom",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "backgroundColor",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "border",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "width",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "height",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "minWidth",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "maxWidth",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "minHeight",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "maxHeight",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "grow",
					kind: PropKind::Value(ValueType::Number),
				},
				PropSchema {
					name: "overflow",
					kind: PropKind::Value(ValueType::String),
				},
				PropSchema {
					name: "cursor",
					kind: PropKind::Value(ValueType::String),
				},
			],
		}),
		_ => None,
	}
}

pub fn is_structural(tag: &str) -> bool {
	matches!(
		tag,
		"For"
			| "If" | "Else"
			| "Scope" | "Page"
			| "Route" | "Switch"
			| "Case" | "Import"
			| "Children"
	)
}

fn layout_props() -> &'static [PropSchema] {
	&[
		PropSchema {
			name: "spacing",
			kind: PropKind::Value(ValueType::Number),
		},
		PropSchema {
			name: "padding",
			kind: PropKind::Value(ValueType::Number),
		},
		PropSchema {
			name: "paddingLeft",
			kind: PropKind::Value(ValueType::Number),
		},
		PropSchema {
			name: "paddingRight",
			kind: PropKind::Value(ValueType::Number),
		},
		PropSchema {
			name: "paddingTop",
			kind: PropKind::Value(ValueType::Number),
		},
		PropSchema {
			name: "paddingBottom",
			kind: PropKind::Value(ValueType::Number),
		},
		PropSchema {
			name: "margin",
			kind: PropKind::Value(ValueType::Number),
		},
		PropSchema {
			name: "marginLeft",
			kind: PropKind::Value(ValueType::Number),
		},
		PropSchema {
			name: "marginRight",
			kind: PropKind::Value(ValueType::Number),
		},
		PropSchema {
			name: "marginTop",
			kind: PropKind::Value(ValueType::Number),
		},
		PropSchema {
			name: "marginBottom",
			kind: PropKind::Value(ValueType::Number),
		},
		PropSchema {
			name: "backgroundColor",
			kind: PropKind::Value(ValueType::String),
		},
		PropSchema {
			name: "border",
			kind: PropKind::Value(ValueType::String),
		},
		PropSchema {
			name: "width",
			kind: PropKind::Value(ValueType::Number),
		},
		PropSchema {
			name: "height",
			kind: PropKind::Value(ValueType::Number),
		},
		PropSchema {
			name: "minWidth",
			kind: PropKind::Value(ValueType::Number),
		},
		PropSchema {
			name: "maxWidth",
			kind: PropKind::Value(ValueType::Number),
		},
		PropSchema {
			name: "minHeight",
			kind: PropKind::Value(ValueType::Number),
		},
		PropSchema {
			name: "maxHeight",
			kind: PropKind::Value(ValueType::Number),
		},
		PropSchema {
			name: "grow",
			kind: PropKind::Value(ValueType::Number),
		},
		PropSchema {
			name: "fill",
			kind: PropKind::Value(ValueType::Bool),
		},
		PropSchema {
			name: "breakWords",
			kind: PropKind::Value(ValueType::Bool),
		},
		PropSchema {
			name: "textAlign",
			kind: PropKind::Value(ValueType::String),
		},
		PropSchema {
			name: "cursor",
			kind: PropKind::Value(ValueType::String),
		},
		PropSchema {
			name: "wrap",
			kind: PropKind::Value(ValueType::Bool),
		},
		PropSchema {
			name: "hresize",
			kind: PropKind::Value(ValueType::Bool),
		},
		PropSchema {
			name: "vresize",
			kind: PropKind::Value(ValueType::Bool),
		},
		PropSchema {
			name: "overflow",
			kind: PropKind::Value(ValueType::String),
		},
	]
}
