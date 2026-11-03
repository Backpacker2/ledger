// The markdown editor: CodeMirror 6 editing the markdown itself, so what you type is exactly what lands in the
// vault. It reads like the rendered page: headings are big, bold is bold, and the marks around them (#, **, `,
// [](…)) fade away except where the cursor is, so they can still be edited. Source mode shows them all instead.
// MarkdownEditor.svelte wraps it.

import { defaultKeymap, history, historyKeymap, indentLess, indentMore } from '@codemirror/commands';
import { insertNewlineContinueMarkup, markdown, markdownLanguage } from '@codemirror/lang-markdown';
import { HighlightStyle, syntaxHighlighting, syntaxTree } from '@codemirror/language';
import {
	Compartment,
	EditorSelection,
	EditorState,
	Facet,
	Prec,
	StateEffect,
	StateField,
	type Extension,
	type Range,
	type StateCommand
} from '@codemirror/state';
import {
	Decoration,
	drawSelection,
	EditorView,
	keymap,
	placeholder as placeholderText,
	ViewPlugin,
	WidgetType,
	type DecorationSet,
	type ViewUpdate
} from '@codemirror/view';
import { tags } from '@lezer/highlight';
import { openUrl } from '@tauri-apps/plugin-opener';
import { isVaultSrc, isWebSrc, type ImageLoad } from '$lib/vaultImages';

type SyntaxNode = ReturnType<typeof syntaxTree>['topNode'];

// ---- looks ---------------------------------------------------------------------------------------
// Colours are the app's theme tokens, so one editor theme serves all six themes. Sizes follow `.md` in app.css.

const highlight = HighlightStyle.define([
	{ tag: tags.heading, fontWeight: '700' },
	{ tag: tags.strong, fontWeight: '700' },
	{ tag: tags.emphasis, fontStyle: 'italic' },
	{ tag: tags.strikethrough, textDecoration: 'line-through' },
	// Link text is coloured by the live preview (`cm-md-link`), which knows a link from words in brackets.
	{ tag: tags.url, color: 'var(--accent)' },
	{ tag: tags.monospace, fontFamily: 'var(--font-mono)', fontSize: '0.9em' },
	{ tag: [tags.processingInstruction, tags.labelName, tags.contentSeparator, tags.comment], color: 'var(--faint)' },
	{ tag: tags.quote, color: 'var(--muted)' },
	{ tag: tags.atom, color: 'var(--accent)' }
]);

const theme = EditorView.theme({
	'&': { color: 'var(--ink)', backgroundColor: 'transparent', maxHeight: 'var(--md-max-height, none)' },
	'&.cm-focused': { outline: 'none' },
	'.cm-scroller': { fontFamily: 'var(--font-sans)', lineHeight: '1.55', overflow: 'auto' },
	'.cm-content': { padding: 'var(--md-pad-y) var(--md-pad-x)', minHeight: 'var(--md-min-height, 0)', caretColor: 'var(--ink)' },
	'.cm-line': { padding: '0' },
	'.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--ink)' },
	'&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection': {
		backgroundColor: 'var(--accent-soft)'
	},
	'.cm-placeholder': { color: 'var(--faint)' },
	'.cm-md-h1': { fontSize: '22px', lineHeight: '1.35' },
	'.cm-md-h2': { fontSize: '17px', lineHeight: '1.4' },
	'.cm-md-h3': { fontSize: '15px' },
	'.cm-md-quote': { borderLeft: '3px solid var(--line)', paddingLeft: '12px', color: 'var(--muted)' },
	'.cm-md-codeblock, .cm-md-table': { fontFamily: 'var(--font-mono)', fontSize: '12.5px' },
	'.cm-md-codeblock': { backgroundColor: 'var(--card-2)', padding: '0 10px' },
	'.cm-md-code': { backgroundColor: 'var(--card-2)', borderRadius: '4px', padding: '1px 3px' },
	'.cm-md-link': { color: 'var(--accent)', textDecoration: 'underline', textDecorationColor: 'var(--accent-soft)', textUnderlineOffset: '2px' },
	'.cm-md-bullet': { color: 'var(--muted)', padding: '0 4px 0 2px' },
	'.cm-md-task': { margin: '0 6px 0 0', verticalAlign: '-2px', accentColor: 'var(--accent)', cursor: 'pointer' },
	'.cm-md-picture img': { maxWidth: '100%', borderRadius: '6px', verticalAlign: 'bottom' },
	'.cm-md-picture.missing': { color: 'var(--faint)', fontStyle: 'italic' },
	'.cm-md-table-widget': { padding: '2px 0 6px', overflowX: 'auto' },
	'&.cm-md-source .cm-scroller': { fontFamily: 'var(--font-mono)', fontSize: '13px' }
});

// ---- live preview ------------------------------------------------------------------------------
// Marks are hidden unless the cursor touches what they belong to: the line, for headings, quotes and bullets; the
// span itself, for emphasis, code and links. An editor without focus hides them all.

const hidden = Decoration.replace({});
const codeSpan = Decoration.mark({ class: 'cm-md-code' });
const linkText = Decoration.mark({ class: 'cm-md-link' });
const lineDecorations: Record<string, Decoration> = {};
const lineClass = (cls: string) => (lineDecorations[cls] ??= Decoration.line({ class: cls }));

class Checkbox extends WidgetType {
	constructor(readonly checked: boolean) {
		super();
	}
	eq(other: Checkbox) {
		return other.checked === this.checked;
	}
	toDOM() {
		const box = document.createElement('input');
		box.type = 'checkbox';
		box.checked = this.checked;
		box.className = 'cm-md-task';
		box.setAttribute('aria-label', this.checked ? 'Done' : 'To do');
		return box;
	}
	ignoreEvent() {
		return false;
	}
}

class Bullet extends WidgetType {
	eq() {
		return true;
	}
	toDOM() {
		const dot = document.createElement('span');
		dot.className = 'cm-md-bullet';
		dot.textContent = '•';
		return dot;
	}
}

const checked = Decoration.replace({ widget: new Checkbox(true) });
const unchecked = Decoration.replace({ widget: new Checkbox(false) });
const bullet = Decoration.replace({ widget: new Bullet() });

// Pictures: web ones always; ones in the vault only where the editor was told where to read them (`loadImage`).
const webOnly: ImageLoad = (src) => Promise.resolve(isWebSrc(src) ? src.trim() : null);
const imageLoad = Facet.define<ImageLoad, ImageLoad>({ combine: (loads) => loads[0] ?? webOnly });

/** A picture, shown once it is read; if it can't be, a note saying so in its place. */
function picture(src: string, alt: string, view: EditorView): HTMLElement {
	const box = document.createElement('span');
	box.className = 'cm-md-picture';
	const img = box.appendChild(document.createElement('img'));
	img.alt = alt;
	const missing = () => {
		box.textContent = `Couldn’t load ${alt || src}`;
		box.classList.add('missing');
	};
	img.onload = () => view.requestMeasure();
	img.onerror = missing;
	view.state
		.facet(imageLoad)(src)
		.then((url) => (url ? (img.src = url) : missing()));
	return box;
}

class Picture extends WidgetType {
	constructor(
		readonly src: string,
		readonly alt: string
	) {
		super();
	}
	eq(other: Picture) {
		return other.src === this.src && other.alt === this.alt;
	}
	toDOM(view: EditorView) {
		return picture(this.src, this.alt, view);
	}
	ignoreEvent() {
		return false;
	}
}

function previewDecorations(view: EditorView): DecorationSet {
	const { state } = view;
	const found: Range<Decoration>[] = [];
	const ranges = view.hasFocus ? state.selection.ranges : [];
	const touched = (from: number, to: number) => ranges.some((r) => r.from <= to && r.to >= from);
	const lineTouched = (pos: number) => {
		const line = state.doc.lineAt(pos);
		return touched(line.from, line.to);
	};
	// A replaced range may not cross a line break, and a mark is hidden together with the space after it.
	const hide = (from: number, to: number, withSpace = false) => {
		if (withSpace && state.sliceDoc(to, to + 1) === ' ') to++;
		if (from < to && !state.sliceDoc(from, to).includes('\n')) found.push(hidden.range(from, to));
	};
	const eachLine = (from: number, to: number, cls: string) => {
		for (let pos = from; pos <= to; ) {
			const line = state.doc.lineAt(pos);
			found.push(lineClass(cls).range(line.from));
			pos = line.to + 1;
		}
	};

	for (const visible of view.visibleRanges) {
		syntaxTree(state).iterate({
			from: visible.from,
			to: visible.to,
			enter: (node) => {
				const parent = node.node.parent;
				const heading = /^ATXHeading(\d)$/.exec(node.name);
				if (heading) {
					eachLine(node.from, node.to, `cm-md-h${heading[1]}`);
					return;
				}
				switch (node.name) {
					case 'SetextHeading1':
					case 'SetextHeading2':
						eachLine(node.from, node.from, `cm-md-h${node.name.at(-1)}`);
						break;
					case 'HeaderMark':
						if (parent?.name.startsWith('ATX') && !lineTouched(node.from)) hide(node.from, node.to, true);
						break;
					case 'Blockquote':
						eachLine(node.from, node.to, 'cm-md-quote');
						break;
					case 'QuoteMark':
						if (!lineTouched(node.from)) hide(node.from, node.to, true);
						break;
					case 'FencedCode':
					case 'CodeBlock':
						eachLine(node.from, node.to, 'cm-md-codeblock');
						break;
					case 'Table':
						eachLine(node.from, node.to, 'cm-md-table');
						break;
					case 'InlineCode':
						found.push(codeSpan.range(node.from, node.to));
						break;
					case 'CodeMark':
					case 'EmphasisMark':
					case 'StrikethroughMark':
						// Code fences stay: hiding them would leave a code block with no visible edge to type at.
						if (parent && parent.name !== 'FencedCode' && !touched(parent.from, parent.to)) hide(node.from, node.to);
						break;
					case 'Link': {
						// Plain `[words]` parse as a link too; only one with an address or a reference is one.
						const marks = node.node.getChildren('LinkMark');
						if (marks.length < 2 || !(node.node.getChild('URL') || node.node.getChild('LinkLabel'))) break;
						if (marks[0].to < marks[1].from) found.push(linkText.range(marks[0].to, marks[1].from));
						if (!touched(node.from, node.to)) {
							hide(node.from, marks[0].to);
							hide(marks[1].from, node.to);
						}
						break;
					}
					case 'Image': {
						// The picture in place of its markdown; with the cursor on it, the markdown and the picture after it.
						const marks = node.node.getChildren('LinkMark');
						const url = node.node.getChild('URL');
						if (marks.length < 2 || !url) break;
						const src = state.sliceDoc(url.from, url.to);
						if (!isWebSrc(src) && !(isVaultSrc(src) && state.facet(imageLoad) !== webOnly)) break;
						const widget = new Picture(src, state.sliceDoc(marks[0].to, marks[1].from));
						if (touched(node.from, node.to)) found.push(Decoration.widget({ widget, side: 1 }).range(node.to));
						else found.push(Decoration.replace({ widget }).range(node.from, node.to));
						return false;
					}
					case 'URL':
						if (parent?.name !== 'Link' && parent?.name !== 'Image') found.push(linkText.range(node.from, node.to));
						break;
					case 'ListMark':
						if (parent?.parent?.name !== 'BulletList' || lineTouched(node.from)) break;
						// A task shows its checkbox instead of a bullet.
						if (parent.getChild('Task')) hide(node.from, node.to, true);
						else found.push(bullet.range(node.from, node.to));
						break;
					case 'TaskMarker':
						if (!touched(node.from, node.to)) {
							const done = state.sliceDoc(node.from + 1, node.to - 1).trim() !== '';
							found.push((done ? checked : unchecked).range(node.from, node.to));
						}
						break;
				}
			}
		});
	}
	return Decoration.set(found, true);
}

/** The URL of the link at `pos`, if it is one the system browser should open. */
function linkAt(state: EditorState, pos: number): string | null {
	for (let node = syntaxTree(state).resolveInner(pos, 1); ; ) {
		const url = node.name === 'URL' ? node : node.name === 'Link' || node.name === 'Autolink' ? node.getChild('URL') : null;
		if (url) {
			const href = state.sliceDoc(url.from, url.to);
			return /^(https?:|mailto:)/i.test(href) ? href : null;
		}
		if (!node.parent) return null;
		node = node.parent;
	}
}

const livePreview = ViewPlugin.fromClass(
	class {
		decorations: DecorationSet;
		constructor(view: EditorView) {
			this.decorations = previewDecorations(view);
		}
		update(update: ViewUpdate) {
			if (
				update.docChanged ||
				update.viewportChanged ||
				update.selectionSet ||
				update.focusChanged ||
				syntaxTree(update.startState) !== syntaxTree(update.state)
			) {
				this.decorations = previewDecorations(update.view);
			}
		}
	},
	{
		decorations: (plugin) => plugin.decorations,
		eventHandlers: {
			mousedown(event, view) {
				const target = event.target as HTMLElement;
				// A checkbox ticks its task off in the text: `[ ]` ⇄ `[x]`.
				if (target instanceof HTMLInputElement && target.classList.contains('cm-md-task')) {
					event.preventDefault();
					const pos = view.posAtDOM(target);
					const done = view.state.sliceDoc(pos + 1, pos + 2).trim() !== '';
					view.dispatch({ changes: { from: pos + 1, to: pos + 2, insert: done ? ' ' : 'x' }, userEvent: 'input' });
					return true;
				}
				// Ctrl/⌘+click opens a link in the system browser, like links in rendered markdown.
				if ((event.ctrlKey || event.metaKey) && target.closest('.cm-md-link')) {
					const href = linkAt(view.state, view.posAtDOM(target));
					if (href) {
						event.preventDefault();
						openUrl(href).catch(() => {});
						return true;
					}
				}
				return false;
			}
		}
	}
);

// ---- tables --------------------------------------------------------------------------------------
// In live preview a table is drawn as one, unless the cursor is in it: then its markdown is there to edit. A table
// takes whole lines, which a view plugin may not replace, so tables come from a state field. The state doesn't know
// about focus, so the field keeps track of it: an editor without focus draws every table.

const focusChanged = StateEffect.define<boolean>();
const inSource = Facet.define<boolean, boolean>({ combine: (values) => values.some(Boolean) });

const inlineTags: Record<string, string> = { Emphasis: 'em', StrongEmphasis: 'strong', Strikethrough: 's', InlineCode: 'code' };
const inlineMarks = new Set(['EmphasisMark', 'StrikethroughMark', 'CodeMark', 'LinkMark']);

function span(cls: string, text = '') {
	const el = document.createElement('span');
	el.className = cls;
	el.textContent = text;
	return el;
}

/** The inline markdown of a cell as elements, without its marks; only `from`…`to` of `node`. */
function inline(text: (from: number, to: number) => string, node: SyntaxNode, into: HTMLElement, view: EditorView, from = node.from, to = node.to) {
	let pos = from;
	for (let child = node.firstChild; child; child = child.nextSibling) {
		if (child.from < from || child.to > to) continue;
		if (child.from > pos) into.append(text(pos, child.from));
		pos = child.to;
		const tag = inlineTags[child.name];
		const marks = child.getChildren('LinkMark');
		const url = child.getChild('URL');
		if (tag) inline(text, child, into.appendChild(document.createElement(tag)), view);
		else if (child.name === 'Link' && marks.length >= 2 && (url || child.getChild('LinkLabel')))
			inline(text, child, into.appendChild(span('cm-md-link')), view, marks[0].to, marks[1].from);
		else if (child.name === 'Image' && marks.length >= 2 && url) into.append(picture(text(url.from, url.to), text(marks[0].to, marks[1].from), view));
		else if (child.name === 'Link' || child.name === 'Image') into.append(text(child.from, child.to)); // `[words]` in brackets
		else if (child.name === 'URL') into.append(span('cm-md-link', text(child.from, child.to)));
		else if (child.name === 'Escape') into.append(text(child.from + 1, child.to));
		else if (!inlineMarks.has(child.name)) inline(text, child, into, view);
	}
	if (to > pos) into.append(text(pos, to));
}

/** A row's cells in order, null for an empty one. The pipes are TableDelimiter children; a leading one ends no cell. */
function cells(row: SyntaxNode): (SyntaxNode | null)[] {
	const found: (SyntaxNode | null)[] = [];
	let cell: SyntaxNode | null = null;
	for (let child = row.firstChild; child; child = child.nextSibling) {
		if (child.name === 'TableCell') cell = child;
		else if (child.name === 'TableDelimiter') {
			if (cell || child.from > row.from) found.push(cell);
			cell = null;
		}
	}
	if (cell) found.push(cell);
	return found;
}

/** Column alignments from the delimiter row: `:--` left, `--:` right, `:-:` centred. */
function alignments(row: string): string[] {
	return row
		.trim()
		.replace(/^\||\|$/g, '')
		.split('|')
		.map((spec) => {
			const s = spec.trim();
			return s.startsWith(':') ? (s.endsWith(':') ? 'center' : 'left') : s.endsWith(':') ? 'right' : '';
		});
}

/** The table in `source` (its lines as written), with the rendered page's table styles (`.md table`). */
function tableDom(source: string, view: EditorView): HTMLElement {
	const wrap = document.createElement('div');
	wrap.className = 'md cm-md-table-widget';
	const cursor = markdownLanguage.parser.parse(source).cursor();
	while (cursor.name !== 'Table') if (!cursor.next()) return span('', source);
	const node = cursor.node;
	const text = (from: number, to: number) => source.slice(from, to);
	const delimiter = node.getChild('TableDelimiter');
	const aligns = delimiter ? alignments(text(delimiter.from, delimiter.to)) : [];
	const table = wrap.appendChild(document.createElement('table'));
	let columns = 0;
	for (let row = node.firstChild; row; row = row.nextSibling) {
		const header = row.name === 'TableHeader';
		if (!header && row.name !== 'TableRow') continue;
		const found = cells(row);
		if (header) columns = found.length;
		const tr = (header ? table.createTHead() : (table.tBodies[0] ?? table.createTBody())).insertRow();
		for (let i = 0; i < columns; i++) {
			const cell = tr.appendChild(document.createElement(header ? 'th' : 'td'));
			if (aligns[i]) cell.style.textAlign = aligns[i];
			if (found[i]) inline(text, found[i]!, cell, view);
		}
	}
	return wrap;
}

class Table extends WidgetType {
	constructor(readonly source: string) {
		super();
	}
	eq(other: Table) {
		return other.source === this.source;
	}
	toDOM(view: EditorView) {
		return tableDom(this.source, view);
	}
	ignoreEvent() {
		return false;
	}
}

function tableDecorations(state: EditorState, focused: boolean): DecorationSet {
	if (state.facet(inSource)) return Decoration.none;
	const found: Range<Decoration>[] = [];
	const ranges = focused ? state.selection.ranges : [];
	syntaxTree(state).iterate({
		enter: (node) => {
			if (node.name === 'Table') {
				const from = state.doc.lineAt(node.from).from;
				const to = state.doc.lineAt(node.to).to;
				if (!ranges.some((r) => r.from <= to && r.to >= from)) {
					found.push(Decoration.replace({ widget: new Table(state.sliceDoc(from, to)), block: true }).range(from, to));
				}
				return false;
			}
			// Tables are blocks: only what holds blocks is worth looking into.
			return /^(Document|Blockquote|BulletList|OrderedList|ListItem)$/.test(node.name);
		}
	});
	return Decoration.set(found);
}

const tables = StateField.define<{ focused: boolean; decorations: DecorationSet }>({
	create: (state) => ({ focused: false, decorations: tableDecorations(state, false) }),
	update(value, tr) {
		let focused = value.focused;
		for (const effect of tr.effects) if (effect.is(focusChanged)) focused = effect.value;
		const stale =
			tr.docChanged ||
			tr.selection ||
			focused !== value.focused ||
			tr.startState.facet(inSource) !== tr.state.facet(inSource) ||
			syntaxTree(tr.startState) !== syntaxTree(tr.state);
		return stale ? { focused, decorations: tableDecorations(tr.state, focused) } : value;
	},
	provide: (field) => EditorView.decorations.from(field, (value) => value.decorations)
});

// ---- modes ---------------------------------------------------------------------------------------
// Live preview, above; or source mode: the markdown exactly as it is, every mark in sight, in the monospace font.
// Switched in place, so the cursor, the selection and undo carry over.

const mode = new Compartment();
const sourceMode = [inSource.of(true), EditorView.editorAttributes.of({ class: 'cm-md-source' })];
const modeExtension = (source: boolean) => (source ? sourceMode : livePreview);

/** Switches between source mode and live preview; it also tells a fresh state (`setState`) whether the editor has focus. */
export function setSourceMode(view: EditorView, source: boolean) {
	view.dispatch({ effects: [mode.reconfigure(modeExtension(source)), focusChanged.of(view.hasFocus)] });
}

// ---- keys ----------------------------------------------------------------------------------------

/** Wraps each selection in `mark`, or unwraps it when it already is; with nothing selected, the cursor lands inside a pair. */
function toggleWrap(mark: string): StateCommand {
	return ({ state, dispatch }) => {
		const n = mark.length;
		const update = state.changeByRange((range) => {
			if (state.sliceDoc(range.from - n, range.from) === mark && state.sliceDoc(range.to, range.to + n) === mark) {
				return {
					changes: [
						{ from: range.from - n, to: range.from },
						{ from: range.to, to: range.to + n }
					],
					range: EditorSelection.range(range.from - n, range.to - n)
				};
			}
			return {
				changes: [
					{ from: range.from, insert: mark },
					{ from: range.to, insert: mark }
				],
				range: EditorSelection.range(range.from + n, range.to + n)
			};
		});
		dispatch(state.update(update, { scrollIntoView: true, userEvent: 'input' }));
		return true;
	};
}

/** `[selection]()`, with the cursor where the next thing to type goes: the address, or the text when none was selected. */
const insertLink: StateCommand = ({ state, dispatch }) => {
	const update = state.changeByRange((range) => {
		const text = state.sliceDoc(range.from, range.to);
		return {
			changes: { from: range.from, to: range.to, insert: `[${text}]()` },
			range: EditorSelection.cursor(range.from + (text ? text.length + 3 : 1))
		};
	});
	dispatch(state.update(update, { scrollIntoView: true, userEvent: 'input' }));
	return true;
};

export interface MarkdownEditorOptions {
	placeholder: string;
	/** The accessible name; the visible label sits outside the editor. */
	label?: string;
	spellcheck: boolean;
	/** Source mode instead of live preview; `setSourceMode` switches it later. */
	source: boolean;
	/** Reads the pictures in the vault (`![](chart.png)`); without it, only web pictures are shown. */
	loadImage?: ImageLoad;
	/** Enter submits and Shift+Enter starts a new line, as in a chat box; otherwise Ctrl/⌘+Enter submits. */
	submitOnEnter: boolean;
	/** Tab indents (lists, mostly) instead of moving to the next field: for full documents. */
	tabIndents: boolean;
	submit: () => void;
	/** Escape: returns whether it was handled, so an unhandled one still reaches a modal. */
	cancel: () => boolean;
	changed: (update: ViewUpdate) => void;
}

export function markdownEditor(options: MarkdownEditorOptions): Extension {
	const submit = () => {
		options.submit();
		return true;
	};
	return [
		// Ctrl/⌘+Enter always submits rather than adding a line; without a handler here, a modal's window listener saves.
		Prec.highest(
			keymap.of([
				{ key: 'Mod-Enter', run: submit },
				...(options.submitOnEnter ? [{ key: 'Enter', run: submit, shift: insertNewlineContinueMarkup }] : []),
				{ key: 'Escape', run: options.cancel },
				{ key: 'Mod-b', run: toggleWrap('**') },
				{ key: 'Mod-i', run: toggleWrap('_') },
				{ key: 'Mod-k', run: insertLink },
				...(options.tabIndents ? [{ key: 'Tab', run: indentMore, shift: indentLess }] : [])
			])
		),
		history(),
		drawSelection(),
		markdown({ base: markdownLanguage }),
		syntaxHighlighting(highlight),
		mode.of(modeExtension(options.source)),
		tables,
		EditorView.focusChangeEffect.of((_, focusing) => focusChanged.of(focusing)),
		options.loadImage ? imageLoad.of(options.loadImage) : [],
		keymap.of([...defaultKeymap, ...historyKeymap]),
		EditorView.lineWrapping,
		placeholderText(options.placeholder),
		EditorView.contentAttributes.of({
			spellcheck: String(options.spellcheck),
			...(options.label ? { 'aria-label': options.label } : {})
		}),
		EditorView.updateListener.of(options.changed),
		theme
	];
}
