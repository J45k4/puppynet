use crate::gui::{FlexDirection, Item, ItemPayload, Layout, SelectOption};

pub fn render_document(item: &Item) -> String {
	render_document_with_app_css(item, false)
}

pub fn render_document_with_app_css(item: &Item, app_css: bool) -> String {
	let mut out = String::new();
	out.push_str("<html><head>");
	out.push_str("<title></title>");
	out.push_str(
		"<meta name=\"viewport\" content=\"width=device-width,initial-scale=1,maximum-scale=1\" />",
	);
	out.push_str("<link rel=\"stylesheet\" href=\"/index.css\"></link>");
	if app_css {
		out.push_str("<link rel=\"stylesheet\" href=\"/app.css\"></link>");
	}
	out.push_str("<script defer src=\"/index.js\"></script>");
	out.push_str("</head>");
	out.push_str(
		"<body style=\"display:flex;flex-direction:row;height:100vh;margin:0;width:100%;\">",
	);
	out.push_str(&render_item(item));
	out.push_str("</body></html>");
	out
}

pub fn render_item(item: &Item) -> String {
	match &item.payload {
		ItemPayload::Layout(layout) => render_layout(item, layout),
		ItemPayload::Text { value } => render_text(item, value),
		ItemPayload::TextInput { value, placeholder } => {
			render_text_input(item, value, placeholder)
		}
		ItemPayload::DatePicker { value, placeholder } => {
			render_date_picker(item, value, placeholder)
		}
		ItemPayload::Textarea { value, placeholder } => render_textarea(item, value, placeholder),
		ItemPayload::Select { value, options } => render_select(item, value, options),
		ItemPayload::Checkbox { checked } => render_checkbox(item, *checked),
		ItemPayload::Slider {
			min,
			max,
			value,
			step,
		} => render_slider(item, *min, *max, *value, *step),
		ItemPayload::Button { title } => render_button(item, title),
		ItemPayload::Link { href, text } => render_link(item, href, text),
		ItemPayload::Table { items } => render_table(item, items),
		ItemPayload::Thead { items } => render_section(item, "thead", items),
		ItemPayload::Tbody { items } => render_section(item, "tbody", items),
		ItemPayload::Tr { items } => render_section(item, "tr", items),
		ItemPayload::Th { item: cell } => render_cell(item, "th", cell),
		ItemPayload::Td { item: cell } => render_cell(item, "td", cell),
		ItemPayload::Img {
			src,
			alt,
			object_fit,
		} => render_image(item, src, alt, object_fit.as_deref()),
		ItemPayload::Video {
			room,
			local,
			autoplay,
			muted,
			controls,
		} => render_media(item, "video", room, *local, *autoplay, *muted, *controls),
		ItemPayload::Audio {
			room,
			local,
			autoplay,
			muted,
			controls,
		} => render_media(item, "audio", room, *local, *autoplay, *muted, *controls),
		ItemPayload::FolderPicker => render_folder_picker(item),
		ItemPayload::FloatingLayout {
			x,
			y,
			width,
			height,
		} => render_floating_layout(item, *x, *y, *width, *height),
		ItemPayload::Modal { body, open } => render_modal(item, body, *open),
		ItemPayload::ThreeView { .. } => render_three_view(item),
		ItemPayload::Custom { name, .. } => render_custom(item, name),
		ItemPayload::None => String::new(),
	}
}

fn render_layout(item: &Item, layout: &Layout) -> String {
	let mut style = StyleBuilder::new();
	style.push("display", "flex");
	style.push(
		"flex-direction",
		match layout.flex {
			FlexDirection::Row => "row",
			FlexDirection::Column => "column",
		},
	);
	if layout.spacing > 0 {
		style.push("gap", &format!("{}px", layout.spacing));
	}
	if layout.wrap {
		style.push("flex-wrap", "wrap");
	}
	if layout.horizontal_resize || layout.vresize || layout.hresize {
		let resize = if (layout.horizontal_resize || layout.hresize) && layout.vresize {
			"both"
		} else if layout.horizontal_resize || layout.hresize {
			"horizontal"
		} else {
			"vertical"
		};
		style.push("resize", resize);
		if item.overflow.is_empty() {
			style.push("overflow", "auto");
		}
	}
	apply_item_styles(item, &mut style);
	let classes = Vec::new();
	let attrs = collect_item_attrs(item);
	let children = render_children(&layout.body);
	render_element("div", &classes, style, &attrs, &children)
}

fn render_text(item: &Item, value: &str) -> String {
	let mut style = StyleBuilder::new();
	apply_item_styles(item, &mut style);
	let classes = Vec::new();
	let attrs = collect_item_attrs(item);
	render_element("span", &classes, style, &attrs, &escape_text(value))
}

fn render_custom(item: &Item, name: &str) -> String {
	let mut style = StyleBuilder::new();
	apply_item_styles(item, &mut style);
	let classes = Vec::new();
	let mut attrs = collect_item_attrs(item);
	attrs.push(("data-wgui-custom".to_string(), "true".to_string()));
	attrs.push(("data-wgui-custom-name".to_string(), name.to_string()));
	render_element("div", &classes, style, &attrs, "")
}

fn render_text_input(item: &Item, value: &str, placeholder: &str) -> String {
	let mut style = StyleBuilder::new();
	apply_item_styles(item, &mut style);
	let classes = Vec::new();
	let mut attrs = collect_item_attrs(item);
	attrs.push(("type".to_string(), "text".to_string()));
	attrs.push(("value".to_string(), escape_attr(value)));
	if !placeholder.is_empty() {
		attrs.push(("placeholder".to_string(), escape_attr(placeholder)));
	}
	render_void_element("input", &classes, style, &attrs)
}

fn render_date_picker(item: &Item, value: &str, placeholder: &str) -> String {
	let mut style = StyleBuilder::new();
	apply_item_styles(item, &mut style);
	let classes = Vec::new();
	let mut attrs = collect_item_attrs(item);
	attrs.push(("type".to_string(), "date".to_string()));
	attrs.push(("value".to_string(), escape_attr(value)));
	if !placeholder.is_empty() {
		attrs.push(("placeholder".to_string(), escape_attr(placeholder)));
	}
	render_void_element("input", &classes, style, &attrs)
}

fn render_textarea(item: &Item, value: &str, placeholder: &str) -> String {
	let mut style = StyleBuilder::new();
	style.push("resize", "none");
	style.push("overflow-y", "hidden");
	style.push("min-height", "20px");
	style.push("line-height", "20px");
	let row_count = value.split('\n').count().max(1);
	style.push("height", &format!("{}px", row_count * 20));
	apply_item_styles(item, &mut style);
	let classes = Vec::new();
	let mut attrs = collect_item_attrs(item);
	if !placeholder.is_empty() {
		attrs.push(("placeholder".to_string(), escape_attr(placeholder)));
	}
	render_element("textarea", &classes, style, &attrs, &escape_text(value))
}

fn render_select(item: &Item, value: &str, options: &[SelectOption]) -> String {
	let mut style = StyleBuilder::new();
	apply_item_styles(item, &mut style);
	let classes = Vec::new();
	let mut children = String::new();
	for option in options {
		let mut attrs = Vec::new();
		attrs.push(("value".to_string(), escape_attr(option.value())));
		if option.value() == value {
			attrs.push(("selected".to_string(), "selected".to_string()));
		}
		children.push_str(&render_element(
			"option",
			&[],
			StyleBuilder::new(),
			&attrs,
			&escape_text(option.name()),
		));
	}
	let attrs = collect_item_attrs(item);
	render_element("select", &classes, style, &attrs, &children)
}

fn render_checkbox(item: &Item, checked: bool) -> String {
	let mut style = StyleBuilder::new();
	apply_item_styles(item, &mut style);
	let classes = Vec::new();
	let mut attrs = collect_item_attrs(item);
	attrs.push(("type".to_string(), "checkbox".to_string()));
	if checked {
		attrs.push(("checked".to_string(), "checked".to_string()));
	}
	render_void_element("input", &classes, style, &attrs)
}

fn render_slider(item: &Item, min: i32, max: i32, value: i32, step: i32) -> String {
	let mut style = StyleBuilder::new();
	apply_item_styles(item, &mut style);
	let classes = Vec::new();
	let mut attrs = collect_item_attrs(item);
	attrs.extend(vec![
		("type".to_string(), "range".to_string()),
		("min".to_string(), min.to_string()),
		("max".to_string(), max.to_string()),
		("value".to_string(), value.to_string()),
		("step".to_string(), step.to_string()),
	]);
	render_void_element("input", &classes, style, &attrs)
}

fn render_button(item: &Item, title: &str) -> String {
	let mut style = StyleBuilder::new();
	apply_item_styles(item, &mut style);
	let classes = Vec::new();
	let attrs = collect_item_attrs(item);
	render_element("button", &classes, style, &attrs, &escape_text(title))
}

fn render_link(item: &Item, href: &str, text: &str) -> String {
	let mut style = StyleBuilder::new();
	style.push("color", "inherit");
	style.push("text-decoration", "none");
	apply_item_styles(item, &mut style);
	let classes = Vec::new();
	let mut attrs = collect_item_attrs(item);
	attrs.push(("href".to_string(), escape_attr(href)));
	render_element("a", &classes, style, &attrs, &escape_text(text))
}

fn render_table(item: &Item, items: &[Item]) -> String {
	let mut style = StyleBuilder::new();
	apply_item_styles(item, &mut style);
	let classes = Vec::new();
	let attrs = collect_item_attrs(item);
	let children = render_children(items);
	render_element("table", &classes, style, &attrs, &children)
}

fn render_section(item: &Item, tag: &str, items: &[Item]) -> String {
	let mut style = StyleBuilder::new();
	apply_item_styles(item, &mut style);
	let attrs = collect_item_attrs(item);
	let children = render_children(items);
	render_element(tag, &[], style, &attrs, &children)
}

fn render_cell(item: &Item, tag: &str, child: &Item) -> String {
	let mut style = StyleBuilder::new();
	apply_item_styles(item, &mut style);
	let attrs = collect_item_attrs(item);
	let child_html = render_item(child);
	render_element(tag, &[], style, &attrs, &child_html)
}

fn render_image(item: &Item, src: &str, alt: &str, object_fit: Option<&str>) -> String {
	let mut style = StyleBuilder::new();
	style.push("max-width", "100%");
	style.push("max-height", "100%");
	if let Some(fit) = object_fit {
		style.push("object-fit", fit);
	} else {
		style.push("object-fit", "contain");
	}
	apply_item_styles(item, &mut style);
	let classes = Vec::new();
	let mut attrs = collect_item_attrs(item);
	attrs.push(("src".to_string(), escape_attr(src)));
	attrs.push(("alt".to_string(), escape_attr(alt)));
	attrs.push(("loading".to_string(), "lazy".to_string()));
	render_void_element("img", &classes, style, &attrs)
}

fn render_media(
	item: &Item,
	tag: &str,
	room: &str,
	local: bool,
	autoplay: bool,
	muted: bool,
	controls: bool,
) -> String {
	let mut style = StyleBuilder::new();
	apply_item_styles(item, &mut style);
	let classes = Vec::new();
	let mut attrs = collect_item_attrs(item);
	attrs.push(("data-wgui-rtc".to_string(), escape_attr(tag)));
	attrs.push(("data-wgui-rtc-room".to_string(), escape_attr(room)));
	attrs.push((
		"data-wgui-rtc-local".to_string(),
		if local { "1" } else { "0" }.to_string(),
	));
	if autoplay {
		attrs.push(("autoplay".to_string(), "autoplay".to_string()));
	}
	if muted {
		attrs.push(("muted".to_string(), "muted".to_string()));
	}
	if controls {
		attrs.push(("controls".to_string(), "controls".to_string()));
	}
	if tag == "video" {
		attrs.push(("playsinline".to_string(), "playsinline".to_string()));
	}
	render_element(tag, &classes, style, &attrs, "")
}

fn render_folder_picker(item: &Item) -> String {
	let mut style = StyleBuilder::new();
	apply_item_styles(item, &mut style);
	let mut attrs = collect_item_attrs(item);
	attrs.extend(vec![
		("type".to_string(), "file".to_string()),
		("webkitdirectory".to_string(), "true".to_string()),
	]);
	render_void_element("input", &[], style, &attrs)
}

fn render_floating_layout(item: &Item, x: u32, y: u32, width: u32, height: u32) -> String {
	let mut style = StyleBuilder::new();
	style.push("position", "absolute");
	style.push("left", &format!("{}px", x));
	style.push("top", &format!("{}px", y));
	style.push("width", &format!("{}px", width));
	style.push("height", &format!("{}px", height));
	apply_item_styles(item, &mut style);
	let attrs = collect_item_attrs(item);
	render_element("div", &[], style, &attrs, "")
}

fn render_modal(item: &Item, body: &[Item], open: bool) -> String {
	let mut style = StyleBuilder::new();
	style.push("position", "fixed");
	style.push("left", "0");
	style.push("top", "0");
	style.push("width", "100vw");
	style.push("height", "100vh");
	style.push("display", if open { "flex" } else { "none" });
	style.push("align-items", "center");
	style.push("justify-content", "center");
	style.push("padding", "32px");
	style.push("box-sizing", "border-box");
	style.push("background-color", "rgba(0, 0, 0, 0.45)");
	style.push("backdrop-filter", "blur(2px)");
	style.push("z-index", "1000");
	style.push("pointer-events", if open { "auto" } else { "none" });
	apply_item_styles(item, &mut style);
	let children = render_children(body);
	let mut attrs = collect_item_attrs(item);
	attrs.extend(vec![
		("data-modal".to_string(), "overlay".to_string()),
		("role".to_string(), "dialog".to_string()),
		("aria-modal".to_string(), "true".to_string()),
		(
			"aria-hidden".to_string(),
			if open {
				"false".to_string()
			} else {
				"true".to_string()
			},
		),
	]);
	render_element("div", &[], style, &attrs, &children)
}

fn render_three_view(item: &Item) -> String {
	let mut style = StyleBuilder::new();
	style.push("display", "block");
	style.push("width", "100%");
	style.push("height", "100%");
	apply_item_styles(item, &mut style);
	let classes = Vec::new();
	let mut attrs = collect_item_attrs(item);
	attrs.push(("data-wgui-three".to_string(), "true".to_string()));
	render_element("canvas", &classes, style, &attrs, "")
}

fn render_children(items: &[Item]) -> String {
	let mut out = String::new();
	for item in items {
		out.push_str(&render_item(item));
	}
	out
}

fn render_element(
	tag: &str,
	classes: &[String],
	style: StyleBuilder,
	attrs: &[(String, String)],
	children: &str,
) -> String {
	let mut out = String::new();
	out.push('<');
	out.push_str(tag);
	out.push_str(&render_attributes(classes, style, attrs));
	out.push('>');
	out.push_str(children);
	out.push_str("</");
	out.push_str(tag);
	out.push('>');
	out
}

fn render_void_element(
	tag: &str,
	classes: &[String],
	style: StyleBuilder,
	attrs: &[(String, String)],
) -> String {
	let mut out = String::new();
	out.push('<');
	out.push_str(tag);
	out.push_str(&render_attributes(classes, style, attrs));
	out.push_str(" />");
	out
}

fn render_attributes(
	classes: &[String],
	style: StyleBuilder,
	attrs: &[(String, String)],
) -> String {
	let mut out = String::new();
	if !classes.is_empty() {
		out.push_str(" class=\"");
		out.push_str(&classes.join(" "));
		out.push('"');
	}
	if let Some(style_value) = style.build() {
		out.push_str(" style=\"");
		out.push_str(&escape_attr(&style_value));
		out.push('"');
	}
	for (name, value) in attrs {
		out.push(' ');
		out.push_str(name);
		out.push_str("=\"");
		out.push_str(&escape_attr(value));
		out.push('"');
	}
	out
}

fn apply_item_styles(item: &Item, style: &mut StyleBuilder) {
	if item.fill {
		style.push("width", "100%");
		style.push("box-sizing", "border-box");
	}
	if item.width > 0 {
		style.push("width", &format!("{}px", item.width));
	}
	if item.height > 0 {
		style.push("height", &format!("{}px", item.height));
	}
	if item.min_width > 0 {
		style.push("min-width", &format!("{}px", item.min_width));
	}
	if item.max_width > 0 {
		style.push("max-width", &format!("{}px", item.max_width));
	}
	if item.min_height > 0 {
		style.push("min-height", &format!("{}px", item.min_height));
	}
	if item.max_height > 0 {
		style.push("max-height", &format!("{}px", item.max_height));
	}
	if item.grow > 0 {
		style.push("flex-grow", &item.grow.to_string());
	}
	if !item.background_color.is_empty() {
		style.push("background-color", &item.background_color);
	}
	if !item.color.is_empty() {
		style.push("color", &item.color);
	}
	if item.break_words {
		style.push("overflow-wrap", "anywhere");
		style.push("word-break", "break-word");
	}
	if !item.text_align.is_empty() {
		style.push("text-align", &item.text_align);
	}
	if !item.cursor.is_empty() {
		style.push("cursor", &item.cursor);
	}
	if item.margin > 0 {
		style.push("margin", &format!("{}px", item.margin));
	}
	if item.margin_left > 0 {
		style.push("margin-left", &format!("{}px", item.margin_left));
	}
	if item.margin_right > 0 {
		style.push("margin-right", &format!("{}px", item.margin_right));
	}
	if item.margin_top > 0 {
		style.push("margin-top", &format!("{}px", item.margin_top));
	}
	if item.margin_bottom > 0 {
		style.push("margin-bottom", &format!("{}px", item.margin_bottom));
	}
	if item.padding > 0 {
		style.push("padding", &format!("{}px", item.padding));
	}
	if item.padding_left > 0 {
		style.push("padding-left", &format!("{}px", item.padding_left));
	}
	if item.padding_right > 0 {
		style.push("padding-right", &format!("{}px", item.padding_right));
	}
	if item.padding_top > 0 {
		style.push("padding-top", &format!("{}px", item.padding_top));
	}
	if item.padding_bottom > 0 {
		style.push("padding-bottom", &format!("{}px", item.padding_bottom));
	}
	if !item.border.is_empty() {
		style.push("border", &item.border);
	}
	if !item.overflow.is_empty() {
		style.push("overflow", &item.overflow);
	}
}

fn collect_item_attrs(item: &Item) -> Vec<(String, String)> {
	let mut attrs = Vec::new();
	if item.editable {
		attrs.push(("contenteditable".to_string(), "true".to_string()));
	}
	attrs
}

fn escape_text(input: &str) -> String {
	input
		.replace('&', "&amp;")
		.replace('<', "&lt;")
		.replace('>', "&gt;")
}

fn escape_attr(input: &str) -> String {
	input
		.replace('&', "&amp;")
		.replace('<', "&lt;")
		.replace('>', "&gt;")
		.replace('"', "&quot;")
		.replace('\'', "&#39;")
}

struct StyleBuilder {
	entries: Vec<String>,
}

impl StyleBuilder {
	fn new() -> Self {
		Self {
			entries: Vec::new(),
		}
	}

	fn push(&mut self, name: &str, value: &str) {
		if value.is_empty() {
			return;
		}
		self.entries.push(format!("{}:{}", name, value));
	}

	fn build(self) -> Option<String> {
		if self.entries.is_empty() {
			None
		} else {
			Some(self.entries.join(";"))
		}
	}
}
