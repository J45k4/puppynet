use anyhow::{anyhow, Context, Result};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaAst {
	pub models: Vec<ModelAst>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelAst {
	pub name: String,
	pub fields: Vec<FieldAst>,
	pub attributes: Vec<AttributeAst>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldAst {
	pub name: String,
	pub ty: TypeAst,
	pub attributes: Vec<AttributeAst>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeAst {
	pub name: String,
	pub is_optional: bool,
	pub is_list: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeAst {
	pub name: String,
	pub args: Option<String>,
}

pub fn parse_schema_file<P: AsRef<Path>>(path: P) -> Result<SchemaAst> {
	let path = path.as_ref();
	let input = std::fs::read_to_string(path)
		.with_context(|| format!("failed reading schema file {}", path.display()))?;
	parse_schema(&input).with_context(|| format!("failed parsing schema file {}", path.display()))
}

pub fn parse_schema(input: &str) -> Result<SchemaAst> {
	let mut models = Vec::new();
	let mut current: Option<ModelAst> = None;

	for (idx, raw_line) in input.lines().enumerate() {
		let line_no = idx + 1;
		let line = strip_comments(raw_line).trim();
		if line.is_empty() {
			continue;
		}

		if let Some(model) = current.as_mut() {
			if line == "}" {
				models.push(current.take().expect("model should exist"));
				continue;
			}

			if line.starts_with("@@") {
				let attrs = parse_attributes(line, line_no, true)?;
				model.attributes.extend(attrs);
				continue;
			}

			model.fields.push(parse_field(line, line_no)?);
			continue;
		}

		if line.starts_with("model ") {
			let after = line.trim_start_matches("model ").trim();
			let Some(head) = after.strip_suffix('{') else {
				return Err(anyhow!(
					"line {line_no}: model declaration must end with `{{`"
				));
			};
			let name = head.trim();
			if name.is_empty() {
				return Err(anyhow!("line {line_no}: model name missing"));
			}
			if name.contains(char::is_whitespace) {
				return Err(anyhow!(
					"line {line_no}: model name must be a single identifier"
				));
			}
			current = Some(ModelAst {
				name: name.to_string(),
				fields: Vec::new(),
				attributes: Vec::new(),
			});
			continue;
		}

		return Err(anyhow!("line {line_no}: unexpected content `{line}`"));
	}

	if current.is_some() {
		return Err(anyhow!("schema ended while parsing model block"));
	}

	Ok(SchemaAst { models })
}

pub fn to_diff_schema(schema: &SchemaAst) -> crate::schema_diff::Schema {
	let tables = schema
		.models
		.iter()
		.map(|model| crate::schema_diff::TableSchema {
			name: model.name.clone(),
			columns: model
				.fields
				.iter()
				.map(|field| crate::schema_diff::ColumnSchema {
					name: field.name.clone(),
					rust_type: type_to_string(&field.ty),
				})
				.collect(),
		})
		.collect();
	crate::schema_diff::Schema { tables }
}

fn parse_field(line: &str, line_no: usize) -> Result<FieldAst> {
	let Some((name_raw, rest_raw)) = line.split_once(':') else {
		return Err(anyhow!("line {line_no}: expected `name: Type`"));
	};

	let name = name_raw.trim();
	if name.is_empty() {
		return Err(anyhow!("line {line_no}: field name cannot be empty"));
	}

	let rest = rest_raw.trim();
	if rest.is_empty() {
		return Err(anyhow!("line {line_no}: field type is missing"));
	}

	let ty_end = rest
		.find(|c: char| c.is_whitespace() || c == '@')
		.unwrap_or(rest.len());
	let type_token = &rest[..ty_end];
	let ty = parse_type(type_token, line_no)?;

	let attrs_str = rest[ty_end..].trim();
	let attributes = parse_attributes(attrs_str, line_no, false)?;

	Ok(FieldAst {
		name: name.to_string(),
		ty,
		attributes,
	})
}

fn parse_type(token: &str, line_no: usize) -> Result<TypeAst> {
	if token.is_empty() {
		return Err(anyhow!("line {line_no}: field type is missing"));
	}

	let mut base = token;
	let mut is_optional = false;
	let mut is_list = false;

	if let Some(stripped) = base.strip_suffix('?') {
		is_optional = true;
		base = stripped;
	}
	if let Some(stripped) = base.strip_suffix("[]") {
		is_list = true;
		base = stripped;
	}

	if base.is_empty() {
		return Err(anyhow!("line {line_no}: invalid field type `{token}`"));
	}

	Ok(TypeAst {
		name: base.to_string(),
		is_optional,
		is_list,
	})
}

fn parse_attributes(
	input: &str,
	line_no: usize,
	allow_double_at: bool,
) -> Result<Vec<AttributeAst>> {
	let mut out = Vec::new();
	let bytes = input.as_bytes();
	let mut i = 0usize;

	while i < bytes.len() {
		while i < bytes.len() && bytes[i].is_ascii_whitespace() {
			i += 1;
		}
		if i >= bytes.len() {
			break;
		}

		if bytes[i] != b'@' {
			return Err(anyhow!("line {line_no}: invalid attribute syntax"));
		}

		let mut at_len = 1usize;
		if i + 1 < bytes.len() && bytes[i + 1] == b'@' {
			if !allow_double_at {
				return Err(anyhow!(
					"line {line_no}: field attributes must use single `@`"
				));
			}
			at_len = 2;
		}
		i += at_len;

		let name_start = i;
		while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
			i += 1;
		}
		if i == name_start {
			return Err(anyhow!("line {line_no}: attribute name missing"));
		}
		let name = &input[name_start..i];

		while i < bytes.len() && bytes[i].is_ascii_whitespace() {
			i += 1;
		}

		let args = if i < bytes.len() && bytes[i] == b'(' {
			let start = i + 1;
			let mut depth = 1i32;
			i += 1;
			while i < bytes.len() && depth > 0 {
				match bytes[i] {
					b'(' => depth += 1,
					b')' => depth -= 1,
					_ => {}
				}
				i += 1;
			}
			if depth != 0 {
				return Err(anyhow!("line {line_no}: unclosed attribute arguments"));
			}
			Some(input[start..i - 1].trim().to_string())
		} else {
			None
		};

		out.push(AttributeAst {
			name: name.to_string(),
			args,
		});
	}

	Ok(out)
}

fn strip_comments(line: &str) -> &str {
	match line.split_once("//") {
		Some((left, _)) => left,
		None => line,
	}
}

fn type_to_string(ty: &TypeAst) -> String {
	let mut out = ty.name.clone();
	if ty.is_list {
		out.push_str("[]");
	}
	if ty.is_optional {
		out.push('?');
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_membership_schema() {
		let src = r#"
model Membership {
  id: UUID @id

  userId: UUID
  chatId: UUID

  canRead: Bool @default(true)
  canWrite: Bool @default(true)
  canAddUsers: Bool @default(false)
  canRemoveUsers: Bool @default(false)

  user: User @relation(userId)
  chat: Chat @relation(chatId)
}
"#;

		let schema = parse_schema(src).expect("parse membership schema");
		assert_eq!(schema.models.len(), 1);
		let model = &schema.models[0];
		assert_eq!(model.name, "Membership");
		assert_eq!(model.fields.len(), 9);
		assert_eq!(model.fields[0].name, "id");
		assert_eq!(model.fields[0].ty.name, "UUID");
		assert_eq!(model.fields[0].attributes[0].name, "id");
		assert_eq!(model.fields[3].name, "canRead");
		assert_eq!(model.fields[3].attributes[0].name, "default");
		assert_eq!(model.fields[7].name, "user");
		assert_eq!(model.fields[7].attributes[0].name, "relation");
	}

	#[test]
	fn parses_optional_and_list_types() {
		let src = r#"
model Example {
  tags: String[]
  ownerId: UUID?
}
"#;
		let schema = parse_schema(src).expect("parse type flags");
		let fields = &schema.models[0].fields;
		assert!(fields[0].ty.is_list);
		assert!(!fields[0].ty.is_optional);
		assert!(!fields[1].ty.is_list);
		assert!(fields[1].ty.is_optional);
	}

	#[test]
	fn errors_on_unclosed_model() {
		let src = "model Broken {\n  id: UUID @id\n";
		let err = parse_schema(src).expect_err("should fail");
		assert!(err.to_string().contains("schema ended"));
	}

	#[test]
	fn parses_model_level_attributes() {
		let src = r#"
model Membership {
  id: UUID @id
  @@index([id])
}
"#;
		let schema = parse_schema(src).expect("parse model attributes");
		let attrs = &schema.models[0].attributes;
		assert_eq!(attrs.len(), 1);
		assert_eq!(attrs[0].name, "index");
		assert_eq!(attrs[0].args.as_deref(), Some("[id]"));
	}
}
