export type TreeNode<T = unknown> = {
	id: string
	label: string
	sublabel?: string
	badge?: string
	children?: TreeNode<T>[]
	data: T
}

type TreeViewOptions<T> = {
	nodes: TreeNode<T>[]
	className?: string
	expanded?: Set<string>
	onSelect?: (node: TreeNode<T>) => void
	renderRow?: (
		node: TreeNode<T>,
		depth: number,
		expanded: boolean,
		hasChildren: boolean,
	) => HTMLElement
}

const defaultRow = <T>(
	node: TreeNode<T>,
	depth: number,
	expanded: boolean,
	hasChildren: boolean,
): HTMLElement => {
	const row = document.createElement("button")
	row.type = "button"
	row.className = "tree-row tree-row--default"
	row.style.setProperty("--tree-depth", String(depth))
	row.setAttribute("data-tree-id", node.id)
	row.innerHTML = `
		<span class="tree-toggle">
			${
				hasChildren
					? `<span class="tree-toggle-btn" data-tree-toggle="${node.id}">${
							expanded ? "▾" : "▸"
					  }</span>`
					: `<span class="tree-toggle-placeholder"></span>`
			}
		</span>
		<span class="tree-body">
			<span class="tree-label">${node.label}</span>
			${node.sublabel ? `<span class="tree-sublabel">${node.sublabel}</span>` : ""}
		</span>
		${node.badge ? `<span class="badge small tree-badge">${node.badge}</span>` : ""}
	`
	return row
}

export const createTreeView = <T>(options: TreeViewOptions<T>) => {
	let nodes = options.nodes
	const expanded = options.expanded ?? new Set<string>()
	let nodeById = new Map<string, TreeNode<T>>()

	const root = document.createElement("div")
	root.className = `tree-view${options.className ? ` ${options.className}` : ""}`

	const buildMap = (list: TreeNode<T>[]) => {
		nodeById = new Map()
		const walk = (items: TreeNode<T>[]) => {
			for (const item of items) {
				nodeById.set(item.id, item)
				if (item.children?.length) {
					walk(item.children)
				}
			}
		}
		walk(list)
	}

	const renderNodes = (list: TreeNode<T>[], depth: number) => {
		for (const node of list) {
			const hasChildren = Boolean(node.children && node.children.length > 0)
			const isExpanded = expanded.has(node.id)
			const row = options.renderRow
				? options.renderRow(node, depth, isExpanded, hasChildren)
				: defaultRow(node, depth, isExpanded, hasChildren)
			if (!row.getAttribute("data-tree-id")) {
				row.setAttribute("data-tree-id", node.id)
			}
			row.style.setProperty("--tree-depth", String(depth))
			root.appendChild(row)
			if (hasChildren && isExpanded) {
				renderNodes(node.children!, depth + 1)
			}
		}
	}

	const render = () => {
		root.innerHTML = ""
		renderNodes(nodes, 0)
	}

	const toggleNode = (id: string) => {
		if (expanded.has(id)) {
			expanded.delete(id)
		} else {
			expanded.add(id)
		}
		render()
	}

	root.addEventListener("click", (event) => {
		const target = event.target as HTMLElement | null
		if (!target) return
		const toggleEl = target.closest<HTMLElement>("[data-tree-toggle]")
		if (toggleEl) {
			const id = toggleEl.getAttribute("data-tree-toggle")
			if (id) {
				event.stopPropagation()
				toggleNode(id)
			}
			return
		}
		const rowEl = target.closest<HTMLElement>("[data-tree-id]")
		if (!rowEl) return
		const id = rowEl.getAttribute("data-tree-id")
		if (!id) return
		const node = nodeById.get(id)
		if (!node) return
		options.onSelect?.(node)
	})

	buildMap(nodes)
	render()

	return {
		element: root,
		expanded,
		toggle: toggleNode,
		setNodes: (next: TreeNode<T>[]) => {
			nodes = next
			buildMap(nodes)
			render()
		},
	}
}
