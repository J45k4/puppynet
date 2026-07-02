use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RouteScore {
	pub static_segments: usize,
	pub total_segments: usize,
	pub dynamic_segments: usize,
	pub exact: bool,
}

#[derive(Debug, Clone)]
pub struct RouteMatch {
	pub params: HashMap<String, String>,
	pub score: RouteScore,
}

#[derive(Debug, Clone)]
pub struct RoutePattern {
	raw: String,
	segments: Vec<String>,
}

impl RoutePattern {
	pub fn parse(route: &str) -> Self {
		Self {
			raw: route.to_string(),
			segments: route_segments(route)
				.into_iter()
				.map(ToString::to_string)
				.collect(),
		}
	}

	pub fn raw(&self) -> &str {
		&self.raw
	}

	pub fn match_path(&self, path: &str) -> Option<RouteMatch> {
		let path_parts = route_segments(path);
		let mut params = HashMap::new();
		let mut static_segments = 0;
		let mut dynamic_segments = 0;
		let mut wildcard_at = None;

		for (index, segment) in self.segments.iter().enumerate() {
			if is_wildcard(segment) {
				wildcard_at = Some(index);
				break;
			}
		}

		let end = wildcard_at.unwrap_or(self.segments.len());
		if wildcard_at.is_none() && end != path_parts.len() {
			return None;
		}
		if wildcard_at.is_some() && path_parts.len() < end {
			return None;
		}

		for (route_seg, path_seg) in self.segments.iter().take(end).zip(path_parts.iter()) {
			if let Some(name) = param_name(route_seg) {
				params.insert(name.to_string(), (*path_seg).to_string());
				dynamic_segments += 1;
			} else if route_seg == path_seg {
				static_segments += 1;
			} else {
				return None;
			}
		}

		Some(RouteMatch {
			params,
			score: RouteScore {
				static_segments,
				total_segments: end,
				dynamic_segments,
				exact: wildcard_at.is_none(),
			},
		})
	}
}

pub fn route_params(route: &str, path: &str) -> Option<HashMap<String, String>> {
	RoutePattern::parse(route)
		.match_path(path)
		.map(|matched| matched.params)
}

pub fn best_route_index<T, F>(routes: &[T], path: &str, route: F) -> Option<usize>
where
	F: Fn(&T) -> &RoutePattern,
{
	let mut best = None;
	for (index, candidate) in routes.iter().enumerate() {
		let Some(matched) = route(candidate).match_path(path) else {
			continue;
		};
		if best.map(|(_, score)| matched.score > score).unwrap_or(true) {
			best = Some((index, matched.score));
		}
	}
	best.map(|(index, _)| index)
}

fn route_segments(path: &str) -> Vec<&str> {
	path.trim_matches('/')
		.split('/')
		.filter(|segment| !segment.is_empty())
		.collect()
}

fn is_wildcard(segment: &str) -> bool {
	segment == "*" || segment == "{*wildcard}"
}

fn param_name(segment: &str) -> Option<&str> {
	if let Some(name) = segment.strip_prefix(':') {
		if !name.is_empty() {
			return Some(name);
		}
	}
	if segment.starts_with('{') && segment.ends_with('}') {
		let inner = &segment[1..segment.len() - 1];
		if !inner.is_empty() && !inner.starts_with('*') {
			return Some(inner);
		}
	}
	None
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn static_route_beats_dynamic_route() {
		let routes = vec![
			RoutePattern::parse("/posts/:post_id"),
			RoutePattern::parse("/posts/new"),
		];

		assert_eq!(
			best_route_index(&routes, "/posts/new", |route| route),
			Some(1)
		);
	}

	#[test]
	fn dynamic_route_extracts_params() {
		let matched = RoutePattern::parse("/posts/:post_id")
			.match_path("/posts/123")
			.unwrap();

		assert_eq!(
			matched.params.get("post_id").map(String::as_str),
			Some("123")
		);
	}

	#[test]
	fn dynamic_route_can_have_static_suffix() {
		let matched = RoutePattern::parse("/posts/{post_id}/edit")
			.match_path("/posts/123/edit")
			.unwrap();

		assert_eq!(
			matched.params.get("post_id").map(String::as_str),
			Some("123")
		);
	}

	#[test]
	fn wildcard_route_is_fallback() {
		let routes = vec![RoutePattern::parse("/*"), RoutePattern::parse("/posts/:id")];

		assert_eq!(
			best_route_index(&routes, "/posts/123", |route| route),
			Some(1)
		);
		assert_eq!(
			best_route_index(&routes, "/missing", |route| route),
			Some(0)
		);
	}

	#[test]
	fn route_pattern_is_exact_without_wildcard() {
		assert!(RoutePattern::parse("/posts/:id")
			.match_path("/posts/123/edit")
			.is_none());
	}
}
