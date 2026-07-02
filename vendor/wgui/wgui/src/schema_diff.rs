use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
	pub tables: Vec<TableSchema>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSchema {
	pub name: String,
	pub columns: Vec<ColumnSchema>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSchema {
	pub name: String,
	pub rust_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffOp {
	CreateTable { table: TableSchema },
	AddColumn { table: String, column: ColumnSchema },
}

pub fn diff_schemas(current: &Schema, target: &Schema) -> Vec<DiffOp> {
	let current_map: HashMap<&str, &TableSchema> = current
		.tables
		.iter()
		.map(|t| (t.name.as_str(), t))
		.collect();

	let mut ops = Vec::new();
	for table in &target.tables {
		match current_map.get(table.name.as_str()) {
			None => ops.push(DiffOp::CreateTable {
				table: table.clone(),
			}),
			Some(existing) => {
				let existing_cols: HashMap<&str, &ColumnSchema> = existing
					.columns
					.iter()
					.map(|c| (c.name.as_str(), c))
					.collect();
				for col in &table.columns {
					if existing_cols.contains_key(col.name.as_str()) {
						continue;
					}
					ops.push(DiffOp::AddColumn {
						table: table.name.clone(),
						column: col.clone(),
					});
				}
			}
		}
	}

	ops
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn creates_new_table_when_missing() {
		let current = Schema { tables: vec![] };
		let target = Schema {
			tables: vec![TableSchema {
				name: "Message".to_string(),
				columns: vec![ColumnSchema {
					name: "body".to_string(),
					rust_type: "String".to_string(),
				}],
			}],
		};

		let ops = diff_schemas(&current, &target);
		assert_eq!(ops.len(), 1);
		assert!(matches!(ops[0], DiffOp::CreateTable { .. }));
	}

	#[test]
	fn adds_only_missing_columns() {
		let current = Schema {
			tables: vec![TableSchema {
				name: "Message".to_string(),
				columns: vec![ColumnSchema {
					name: "body".to_string(),
					rust_type: "String".to_string(),
				}],
			}],
		};
		let target = Schema {
			tables: vec![TableSchema {
				name: "Message".to_string(),
				columns: vec![
					ColumnSchema {
						name: "body".to_string(),
						rust_type: "String".to_string(),
					},
					ColumnSchema {
						name: "author".to_string(),
						rust_type: "String".to_string(),
					},
				],
			}],
		};

		let ops = diff_schemas(&current, &target);
		assert_eq!(ops.len(), 1);
		match &ops[0] {
			DiffOp::AddColumn { table, column } => {
				assert_eq!(table, "Message");
				assert_eq!(column.name, "author");
			}
			_ => panic!("expected add column"),
		}
	}
}
