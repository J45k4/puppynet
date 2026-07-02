use crate::edit_distance::get_minimum_edits;
use crate::edit_distance::EditOperation;
use crate::gui::Item;
use crate::gui::ItemPayload;
use crate::gui::ThreeNode;
use crate::gui::ThreePropValue;
use crate::types::AddBack;
use crate::types::AddFront;
use crate::types::ClientAction;
use crate::types::InsertAt;
use crate::types::ItemPath;
use crate::types::PropKey;
use crate::types::RemoveInx;
use crate::types::Replace;
use crate::types::SetProp;
use crate::types::ThreeOp;
use crate::types::Value;
use std::collections::HashMap;

fn inner_diff(changes: &mut Vec<ClientAction>, old: &Item, new: &Item, path: ItemPath) {
	log::trace!("{:?} inner_dif", path);
	let mut sets: Vec<SetProp> = Vec::new();

	match (&old.payload, &new.payload) {
		(ItemPayload::ThreeView { root: old_root }, ItemPayload::ThreeView { root: new_root }) => {
			let ops = diff_three_nodes(old_root, new_root);
			if !ops.is_empty() {
				changes.push(ClientAction::ThreePatch {
					path: path.clone(),
					ops,
				});
			}
		}
		(ItemPayload::Layout(old_layout), ItemPayload::Layout(new_layout)) => {
			log::trace!("{:?} layout", path);

			if old_layout.flex != new_layout.flex {
				let flex = match new_layout.flex {
					crate::gui::FlexDirection::Row => "row",
					crate::gui::FlexDirection::Column => "column",
				};
				sets.push(SetProp {
					key: PropKey::FlexDirection,
					value: Value::String(flex.to_string()),
				});
			}

			if old_layout.wrap != new_layout.wrap
				|| old_layout.horizontal_resize != new_layout.horizontal_resize
				|| old_layout.vresize != new_layout.vresize
				|| old_layout.hresize != new_layout.hresize
				|| old_layout.pos != new_layout.pos
			{
				changes.push(ClientAction::Replace(Replace {
					path: path.clone(),
					item: new.clone(),
				}));
				return;
			}

			if old_layout.spacing != new_layout.spacing {
				println!("{:?} spacing is different", path);
				sets.push(SetProp {
					key: PropKey::Spacing,
					value: Value::Number(new_layout.spacing),
				})
			}

			let edits = get_minimum_edits(&old_layout.body, &new_layout.body);
			for edit in edits {
				match edit {
					EditOperation::InsertFirst(item) => {
						log::trace!("{:?} insert first", path);

						changes.push(ClientAction::AddFront(AddFront {
							path: path.clone(),
							item: item,
						}));
					}
					EditOperation::InsertAfter(index, item) => {
						log::trace!("{:?} insert after {}", path, index);

						changes.push(ClientAction::InsertAt(InsertAt {
							path: path.clone(),
							inx: index,
							item: item,
						}));
					}
					EditOperation::RemoveAt(index) => {
						log::trace!("{:?} remove at index {}", path, index);

						changes.push(ClientAction::RemoveInx(RemoveInx {
							path: path.clone(),
							inx: index,
						}));
					}
					EditOperation::ReplaceAt(i, item) => {
						log::trace!("{:?} replace at {}", path, i);

						let mut path = path.clone();
						path.push(i);

						log::trace!("{:?} new path: {:?}", path, path);

						inner_diff(changes, &old_layout.body[i], &item, path);
					}
					EditOperation::InsertBack(item) => {
						log::trace!("{:?} insert back", path);

						changes.push(ClientAction::AddBack(AddBack {
							path: path.clone(),
							item,
						}));
					}
				}
			}
		}
		_ => {
			if old != new {
				log::trace!("{:?} old and new are different", path);

				changes.push(ClientAction::Replace(Replace {
					path: path.clone(),
					item: new.clone(),
				}));
			}
		}
	}

	if old.id != new.id {
		sets.push(SetProp {
			key: PropKey::ID,
			value: Value::Number(new.id),
		})
	}
	if old.grow != new.grow {
		sets.push(SetProp {
			key: PropKey::Grow,
			value: Value::Number(new.grow),
		});
	}
	if old.fill != new.fill {
		sets.push(SetProp {
			key: PropKey::Fill,
			value: Value::Number(new.fill as u32),
		});
	}
	if old.width != new.width {
		sets.push(SetProp {
			key: PropKey::Width,
			value: Value::Number(new.width),
		});
	}
	if old.height != new.height {
		sets.push(SetProp {
			key: PropKey::Height,
			value: Value::Number(new.height),
		});
	}
	if old.min_width != new.min_width {
		sets.push(SetProp {
			key: PropKey::MinWidth,
			value: Value::Number(new.min_width),
		});
	}
	if old.max_width != new.max_width {
		sets.push(SetProp {
			key: PropKey::MaxWidth,
			value: Value::Number(new.max_width),
		});
	}
	if old.min_height != new.min_height {
		sets.push(SetProp {
			key: PropKey::MinHeight,
			value: Value::Number(new.min_height),
		});
	}
	if old.max_height != new.max_height {
		sets.push(SetProp {
			key: PropKey::MaxHeight,
			value: Value::Number(new.max_height),
		});
	}
	if old.padding != new.padding {
		sets.push(SetProp {
			key: PropKey::Padding,
			value: Value::Number(new.padding as u32),
		});
	}

	if old.border != new.border {
		println!("{:?} border is different", path);
		sets.push(SetProp {
			key: PropKey::Border,
			value: Value::String(new.border.clone()),
		});
	}
	if old.background_color != new.background_color {
		// changes.push(ClientAction::SetStyle {
		// 	path: path.clone(),
		// 	prop: "background-color".to_string(),
		// 	value: new.background_color.to_string()
		// })
		sets.push(SetProp {
			key: PropKey::BackgroundColor,
			value: Value::String(new.background_color.clone()),
		});
	}
	if old.color != new.color {
		sets.push(SetProp {
			key: PropKey::Color,
			value: Value::String(new.color.clone()),
		});
	}
	if old.break_words != new.break_words {
		sets.push(SetProp {
			key: PropKey::BreakWords,
			value: Value::Number(new.break_words as u32),
		});
	}
	if old.overflow != new.overflow {
		sets.push(SetProp {
			key: PropKey::Overflow,
			value: Value::String(new.overflow.clone()),
		});
	}

	if sets.len() > 0 {
		changes.push(ClientAction::SetProp {
			path: path.clone(),
			sets,
		});
	}

	// match (old, new) {
	//     (Item::View(old), Item::View(new)) => {
	//         log::trace!("{:?} inner_diff calculating view minumum edits", path);

	// 		// if old != new {
	// 		// 	println!("{:?} old and new are different", path);
	// 		// 	if old.background_color != new.background_color {
	// 		// 		println!("{:?} background color is different", path);
	// 		// 	}

	// 		// }

	// 		if old.spacing != new.spacing {
	// 			changes.push(ClientAction::SetStyle {
	// 				path: path.clone(),
	// 				prop: "gap".to_string(),
	// 				value: new.spacing.unwrap_or_default().to_string()
	// 			})
	// 		}
	//     }
	//     _ => {
	//         log::trace!("{:?} comparing old and new", path);

	//         if old != new {
	//             log::trace!("{:?} old and new are different", path);

	//             changes.push(
	//                 ClientAction::Replace(
	//                     Replace {
	//                         path: path.clone(),
	//                         item: new.clone()
	//                     }
	//                 )
	//             );
	//         }
	//     }
	// }
}

#[derive(Clone)]
struct ThreeIndexEntry {
	kind: crate::gui::ThreeKind,
	props: HashMap<String, ThreePropValue>,
	children: Vec<u32>,
	parent: Option<u32>,
}

fn index_three_tree(root: &ThreeNode) -> HashMap<u32, ThreeIndexEntry> {
	let mut index = HashMap::new();
	let mut stack: Vec<(&ThreeNode, Option<u32>)> = vec![(root, None)];
	while let Some((node, parent)) = stack.pop() {
		let mut props = HashMap::new();
		for prop in &node.props {
			props.insert(prop.key.clone(), prop.value.clone());
		}
		let children: Vec<u32> = node.children.iter().map(|child| child.id).collect();
		index.insert(
			node.id,
			ThreeIndexEntry {
				kind: node.kind.clone(),
				props,
				children,
				parent,
			},
		);
		for child in &node.children {
			stack.push((child, Some(node.id)));
		}
	}
	index
}

fn collect_three_nodes<'a>(
	root: &'a ThreeNode,
	depth: usize,
	out: &mut Vec<(usize, &'a ThreeNode)>,
) {
	out.push((depth, root));
	for child in &root.children {
		collect_three_nodes(child, depth + 1, out);
	}
}

fn diff_three_nodes(old_root: &ThreeNode, new_root: &ThreeNode) -> Vec<ThreeOp> {
	let old_index = index_three_tree(old_root);
	let new_index = index_three_tree(new_root);

	let mut ops: Vec<ThreeOp> = Vec::new();

	let mut recreate_ids: HashMap<u32, bool> = HashMap::new();
	for (id, new_entry) in &new_index {
		if let Some(old_entry) = old_index.get(id) {
			if old_entry.kind != new_entry.kind {
				recreate_ids.insert(*id, true);
			}
		}
	}

	let mut old_nodes: Vec<(usize, &ThreeNode)> = Vec::new();
	collect_three_nodes(old_root, 0, &mut old_nodes);
	old_nodes.sort_by(|a, b| b.0.cmp(&a.0));
	for (_, node) in old_nodes {
		if !new_index.contains_key(&node.id) || recreate_ids.contains_key(&node.id) {
			if let Some(old_entry) = old_index.get(&node.id) {
				if let Some(parent_id) = old_entry.parent {
					ops.push(ThreeOp::Detach {
						parent_id,
						child_id: node.id,
					});
				}
			}
			ops.push(ThreeOp::Delete { id: node.id });
		}
	}

	let mut new_nodes: Vec<(usize, &ThreeNode)> = Vec::new();
	collect_three_nodes(new_root, 0, &mut new_nodes);
	new_nodes.sort_by(|a, b| a.0.cmp(&b.0));
	for (_, node) in &new_nodes {
		if !old_index.contains_key(&node.id) || recreate_ids.contains_key(&node.id) {
			ops.push(ThreeOp::Create {
				id: node.id,
				kind: node.kind.clone(),
				props: node.props.clone(),
			});
		}
	}

	for (_, node) in &new_nodes {
		let new_entry = match new_index.get(&node.id) {
			Some(entry) => entry,
			None => continue,
		};

		let old_entry = old_index.get(&node.id);
		let old_parent = old_entry.and_then(|entry| entry.parent);
		let new_parent = new_entry.parent;
		if old_parent != new_parent || recreate_ids.contains_key(&node.id) {
			if let Some(parent_id) = old_parent {
				ops.push(ThreeOp::Detach {
					parent_id,
					child_id: node.id,
				});
			}
			if let Some(parent_id) = new_parent {
				ops.push(ThreeOp::Attach {
					parent_id,
					child_id: node.id,
				});
			}
		}

		if recreate_ids.contains_key(&node.id) {
			continue;
		}

		if let Some(old_entry) = old_entry {
			for (key, new_value) in &new_entry.props {
				let old_value = old_entry.props.get(key);
				if old_value != Some(new_value) {
					ops.push(ThreeOp::SetProp {
						id: node.id,
						key: key.clone(),
						value: new_value.clone(),
					});
				}
			}
			for key in old_entry.props.keys() {
				if !new_entry.props.contains_key(key) {
					ops.push(ThreeOp::UnsetProp {
						id: node.id,
						key: key.clone(),
					});
				}
			}
		}
	}

	ops
}

pub fn diff(old: &Item, new: &Item) -> Vec<ClientAction> {
	log::trace!("diff");
	log::trace!("{:?}", old);
	log::trace!("{:?}", new);
	let mut changes = Vec::new();
	let mut path = Vec::new();
	inner_diff(&mut changes, old, new, path);
	log::debug!("diff changes: {:?}", changes);
	changes
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::gui::hstack;

	#[test]
	fn test_view_metadata_diff() {
		let old = hstack([]);
		let new = hstack([]).spacing(10);

		let changes = super::diff(&old.into(), &new.into());
		println!("{:?}", changes);
	}
}
