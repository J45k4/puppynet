export type MultiSelectOption = {
	value: string
	label?: string
}

export type MultiSelect = {
	element: HTMLElement
	getSelected: () => string[]
	setOptions: (options: MultiSelectOption[]) => void
	setSelected: (values: string[]) => void
	clear: () => void
}

type MultiSelectProps = {
	id?: string
	placeholder?: string
	options?: MultiSelectOption[]
	onChange?: (values: string[]) => void
}

export const createMultiSelect = (props: MultiSelectProps): MultiSelect => {
	const selected = new Set<string>()
	let allOptions: MultiSelectOption[] = props.options ?? []
	const wrapper = document.createElement("div")
	wrapper.className = "multiselect"
	if (props.id) wrapper.id = props.id

	const trigger = document.createElement("button")
	trigger.type = "button"
	trigger.className = "multiselect-trigger"
	const label = document.createElement("span")
	label.textContent = props.placeholder ?? "Select..."
	const caret = document.createElement("span")
	caret.className = "multiselect-caret"
	caret.textContent = "▾"
	trigger.append(label, caret)

	const panel = document.createElement("div")
	panel.className = "multiselect-panel"
	const searchBox = document.createElement("input")
	searchBox.type = "text"
	searchBox.placeholder = "Search..."
	searchBox.className = "multiselect-search"
	const optionsEl = document.createElement("div")
	optionsEl.className = "multiselect-options"
	panel.append(searchBox, optionsEl)

	wrapper.append(trigger, panel)

	let open = false
	const close = () => {
		if (!open) return
		open = false
		wrapper.classList.remove("open")
	}
	const toggle = () => {
		open = !open
		if (open) wrapper.classList.add("open")
		else wrapper.classList.remove("open")
	}

	const updateLabel = () => {
		if (selected.size === 0) {
			label.textContent = props.placeholder ?? "Select..."
		} else if (selected.size <= 2) {
			label.textContent = Array.from(selected).join(", ")
		} else {
			label.textContent = `${selected.size} selected`
		}
	}

	const notify = () => {
		updateLabel()
		if (props.onChange) props.onChange(Array.from(selected))
	}

	trigger.addEventListener("click", (ev) => {
		ev.stopPropagation()
		toggle()
	})
	document.addEventListener("click", (ev) => {
		if (!wrapper.contains(ev.target as Node)) {
			close()
		}
	})

	const renderOptions = (options: MultiSelectOption[]) => {
		optionsEl.innerHTML = ""
		options.forEach((opt) => {
			const row = document.createElement("label")
			row.className = "multiselect-option"
			const checkbox = document.createElement("input")
			checkbox.type = "checkbox"
			checkbox.value = opt.value
			checkbox.checked = selected.has(opt.value)
			const text = document.createElement("span")
			text.textContent = opt.label ?? opt.value
			row.append(checkbox, text)
			checkbox.addEventListener("change", (ev) => {
				const target = ev.target as HTMLInputElement
				if (target.checked) selected.add(opt.value)
				else selected.delete(opt.value)
				notify()
			})
			row.addEventListener("click", (ev) => ev.stopPropagation())
			optionsEl.appendChild(row)
		})
	}

	const applyFilter = () => {
		const query = searchBox.value.trim().toLowerCase()
		if (!query) {
			renderOptions(allOptions)
			return
		}
		renderOptions(
			allOptions.filter((opt) => {
				const label = opt.label ?? opt.value
				return label.toLowerCase().includes(query) || opt.value.toLowerCase().includes(query)
			}),
		)
	}

	const setOptions = (options: MultiSelectOption[]) => {
		allOptions = options
		applyFilter()
		notify()
	}

	const setSelected = (values: string[]) => {
		selected.clear()
		values.forEach((v) => selected.add(v))
		optionsEl
			.querySelectorAll<HTMLInputElement>("input[type=checkbox]")
			.forEach((cb) => {
				cb.checked = selected.has(cb.value)
			})
		notify()
	}

	searchBox.addEventListener("input", () => applyFilter())

	if (props.options) {
		setOptions(allOptions)
	} else {
		updateLabel()
	}

	return {
		element: wrapper,
		getSelected: () => Array.from(selected),
		setOptions,
		setSelected,
		clear: () => setSelected([]),
	}
}
