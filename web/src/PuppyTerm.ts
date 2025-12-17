export type PuppyTermOptions = {
	fontSize?: number
	fontFamily?: string
	cols?: number
	rows?: number
	cursorAlpha?: number
	defaultFg?: string
	defaultBg?: string
}

type Cell = {
	ch: string
	fg: string
	bg: string
}

export class PuppyTerm {
	onData: (data: string) => void = () => {}

	private fontSize: number
	private fontFamily: string
	private cursorAlpha: number
	private defaultFg: string
	private defaultBg: string

	private canvas: HTMLCanvasElement | null = null
	private ctx: CanvasRenderingContext2D | null = null

	private cols: number
	private rows: number
	private charW = 10
	private charH: number

	private fg: string
	private bg: string
	private cx = 0
	private cy = 0

	private buf: Cell[][] = []

	private esc = false
	private csi = false
	private csiBuf = ""

	constructor(opts: PuppyTermOptions = {}) {
		this.fontSize = opts.fontSize ?? 16
		this.fontFamily =
			opts.fontFamily ?? "ui-monospace, Menlo, Consolas, monospace"
		this.cursorAlpha = opts.cursorAlpha ?? 0.35
		this.defaultFg = opts.defaultFg ?? "#ddd"
		this.defaultBg = opts.defaultBg ?? "#111"

		this.fg = this.defaultFg
		this.bg = this.defaultBg

		this.cols = opts.cols ?? 80
		this.rows = opts.rows ?? 24
		this.charH = Math.ceil(this.fontSize * 1.3)

		this.initBuffer()
	}

	open(canvas: HTMLCanvasElement) {
		this.canvas = canvas
		const ctx = canvas.getContext("2d", { alpha: false })
		if (!ctx) throw new Error("PuppyTerm: failed to get 2D context")
		this.ctx = ctx

		ctx.font = `${this.fontSize}px ${this.fontFamily}`
		ctx.textBaseline = "top"

		this.charW = Math.ceil(ctx.measureText("M").width)
		this.charH = Math.ceil(this.fontSize * 1.3)

		this.cols = Math.max(1, Math.floor(canvas.width / this.charW))
		this.rows = Math.max(1, Math.floor(canvas.height / this.charH))

		this.initBuffer()
		this.clear()
		this.render()
		this.installInput()
	}

	resizeToCanvas() {
		if (!this.canvas || !this.ctx) return

		const newCols = Math.max(1, Math.floor(this.canvas.width / this.charW))
		const newRows = Math.max(1, Math.floor(this.canvas.height / this.charH))
		if (newCols === this.cols && newRows === this.rows) return

		const old = this.buf
		const oldCols = this.cols
		const oldRows = this.rows

		this.cols = newCols
		this.rows = newRows
		this.initBuffer()

		const minRows = Math.min(oldRows, this.rows)
			const minCols = Math.min(oldCols, this.cols)
			for (let y = 0; y < minRows; y++) {
				for (let x = 0; x < minCols; x++) {
					this.buf[y]![x] = old[y]![x]!
				}
			}

		this.cx = Math.min(this.cx, this.cols - 1)
		this.cy = Math.min(this.cy, this.rows - 1)
		this.render()
	}

	write(data: string) {
		for (let i = 0; i < data.length; i++) {
			const ch = data[i]!

			if (this.handleAnsiChar(ch)) continue

			if (ch === "\n") {
				this.lineFeed()
				continue
			}
			if (ch === "\r") {
				this.cx = 0
				continue
			}
			if (ch === "\b") {
				this.cx = Math.max(0, this.cx - 1)
				this.putChar(" ")
				continue
			}
			if (ch === "\t") {
				this.tab()
				continue
			}

			this.putChar(ch)
		}
		this.render()
	}

	clear() {
		for (let y = 0; y < this.rows; y++) {
			for (let x = 0; x < this.cols; x++) {
				this.buf[y]![x] = { ch: " ", fg: this.fg, bg: this.bg }
			}
		}
		this.cx = 0
		this.cy = 0
	}

	private initBuffer() {
		const makeCell = (): Cell => ({ ch: " ", fg: this.fg, bg: this.bg })
		this.buf = Array.from({ length: this.rows }, () =>
			Array.from({ length: this.cols }, makeCell),
		)
	}

	private render() {
		const ctx = this.ctx
		if (!ctx) return

			for (let y = 0; y < this.rows; y++) {
				for (let x = 0; x < this.cols; x++) {
					const cell = this.buf[y]![x]!
					const px = x * this.charW
					const py = y * this.charH

					ctx.fillStyle = cell.bg
					ctx.fillRect(px, py, this.charW, this.charH)

				if (cell.ch !== " ") {
					ctx.fillStyle = cell.fg
					ctx.fillText(cell.ch, px, py)
				}
			}
		}

		ctx.globalAlpha = this.cursorAlpha
		ctx.fillStyle = "#fff"
		ctx.fillRect(
			this.cx * this.charW,
			this.cy * this.charH,
			this.charW,
			this.charH,
		)
		ctx.globalAlpha = 1.0
	}

	private putChar(ch: string) {
		if (this.cx >= this.cols) {
			this.cx = 0
			this.lineFeed()
		}
		if (this.cy >= this.rows) {
			this.scrollUp()
			this.cy = this.rows - 1
		}

		this.buf[this.cy]![this.cx] = { ch, fg: this.fg, bg: this.bg }
		this.cx++
	}

	private lineFeed() {
		this.cy++
		if (this.cy >= this.rows) {
			this.scrollUp()
			this.cy = this.rows - 1
		}
	}

	private scrollUp() {
		this.buf.shift()
		const emptyRow = Array.from({ length: this.cols }, (): Cell => ({
			ch: " ",
			fg: this.fg,
			bg: this.bg,
		}))
		this.buf.push(emptyRow)
	}

	private tab() {
		const next = (Math.floor(this.cx / 8) + 1) * 8
		this.cx = Math.min(next, this.cols - 1)
	}

	private handleAnsiChar(ch: string): boolean {
		if (!this.esc) {
			if (ch === "\x1b") {
				this.esc = true
				return true
			}
			return false
		}

		if (!this.csi) {
			if (ch === "[") {
				this.csi = true
				this.csiBuf = ""
				return true
			}
			this.esc = false
			return true
		}

		this.csiBuf += ch

		const code = ch.charCodeAt(0)
		const isFinal = code >= 0x40 && code <= 0x7e
		if (!isFinal) return true

		const final = ch
		const body = this.csiBuf.slice(0, -1)
		this.applyCsi(final, body)

		this.esc = false
		this.csi = false
		this.csiBuf = ""
		return true
	}

	private applyCsi(final: string, body: string) {
		const isPrivate = body.startsWith("?")
		const paramsStr = isPrivate ? body.slice(1) : body
		const params = paramsStr.length
			? paramsStr.split(";").map((s) => (s === "" ? Number.NaN : Number(s)))
			: []

		const p = (idx: number, def: number): number =>
			Number.isFinite(params[idx]) ? (params[idx] as number) : def

		switch (final) {
			case "m":
				if (params.length === 0) {
					this.sgr(0)
					break
				}
				for (const n of params) {
					this.sgr(Number.isFinite(n) ? (n as number) : 0)
				}
				break

			case "H":
			case "f": {
				const row = p(0, 1)
				const col = p(1, 1)
				this.cy = Math.min(this.rows - 1, Math.max(0, row - 1))
				this.cx = Math.min(this.cols - 1, Math.max(0, col - 1))
				break
			}

			case "A":
				this.cy = Math.max(0, this.cy - p(0, 1))
				break
			case "B":
				this.cy = Math.min(this.rows - 1, this.cy + p(0, 1))
				break
			case "C":
				this.cx = Math.min(this.cols - 1, this.cx + p(0, 1))
				break
			case "D":
				this.cx = Math.max(0, this.cx - p(0, 1))
				break

			case "J": {
				const mode = p(0, 0)
				if (mode === 2) {
					this.clear()
				} else if (mode === 0) {
					for (let y = this.cy; y < this.rows; y++) {
						const startX = y === this.cy ? this.cx : 0
						for (let x = startX; x < this.cols; x++) {
							this.buf[y]![x] = { ch: " ", fg: this.fg, bg: this.bg }
						}
					}
				}
				break
			}

			case "K": {
				const mode = p(0, 0)
					if (mode === 2) {
						for (let x = 0; x < this.cols; x++) {
							this.buf[this.cy]![x] = { ch: " ", fg: this.fg, bg: this.bg }
						}
					} else if (mode === 0) {
						for (let x = this.cx; x < this.cols; x++) {
							this.buf[this.cy]![x] = { ch: " ", fg: this.fg, bg: this.bg }
						}
					}
					break
				}

			default:
				break
		}
	}

	private sgr(n: number) {
		if (n === 0) {
			this.fg = this.defaultFg
			this.bg = this.defaultBg
			return
		}

		const basic = [
			"#000",
			"#a00",
			"#0a0",
			"#aa0",
			"#00a",
			"#a0a",
			"#0aa",
			"#aaa",
		]
		const bright = [
			"#555",
			"#f55",
			"#5f5",
			"#ff5",
			"#55f",
			"#f5f",
			"#5ff",
			"#fff",
		]

		if (n >= 30 && n <= 37) {
			const next = basic[n - 30]
			if (next) this.fg = next
		}
		if (n >= 40 && n <= 47) {
			const next = basic[n - 40]
			if (next) this.bg = next
		}
		if (n >= 90 && n <= 97) {
			const next = bright[n - 90]
			if (next) this.fg = next
		}
		if (n >= 100 && n <= 107) {
			const next = bright[n - 100]
			if (next) this.bg = next
		}
	}

	private installInput() {
		const canvas = this.canvas!
		canvas.tabIndex = 0
		canvas.style.outline = "none"

		canvas.addEventListener("mousedown", () => canvas.focus())
		canvas.addEventListener("keydown", (e) => {
			if (
				["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Tab"].includes(
					e.key,
				)
			) {
				e.preventDefault()
			}
			const data = this.keyToData(e)
			if (data) this.onData(data)
		})
	}

	private keyToData(e: KeyboardEvent): string {
		if (e.key === "Enter") return "\r"
		if (e.key === "Backspace") return "\x7f"
		if (e.key === "Tab") return "\t"

		if (e.key === "ArrowUp") return "\x1b[A"
		if (e.key === "ArrowDown") return "\x1b[B"
		if (e.key === "ArrowRight") return "\x1b[C"
		if (e.key === "ArrowLeft") return "\x1b[D"

		if (e.ctrlKey && e.key.length === 1) {
			const k = e.key.toUpperCase()
			const code = k.charCodeAt(0)
			if (code >= 65 && code <= 90) return String.fromCharCode(code - 64)
		}

		if (e.altKey && e.key.length === 1) return "\x1b" + e.key

		if (!e.ctrlKey && !e.metaKey && e.key.length === 1) return e.key

		return ""
	}
}
