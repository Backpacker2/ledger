<script lang="ts">
	import { onMount } from 'svelte';
	import { EditorState, type Extension } from '@codemirror/state';
	import { EditorView } from '@codemirror/view';
	import { markdownEditor } from '$lib/markdownEditor';

	/**
	 * Where markdown is written: CodeMirror on the markdown itself, styled like the rendered page (see markdownEditor.ts).
	 * `onsubmit` runs on Ctrl/⌘+Enter, or on Enter with `submitOnEnter`; without it Ctrl/⌘+Enter only bubbles up,
	 * to a modal's own listener. `oninput` is called for typing, not when `value` is changed from outside.
	 */
	let {
		value = $bindable(''),
		placeholder = '',
		label,
		variant = 'field',
		rows = 3,
		minHeight,
		autofocus = false,
		spellcheck = true,
		submitOnEnter = false,
		onsubmit,
		oncancel,
		oninput
	}: {
		value?: string;
		placeholder?: string;
		label?: string;
		/** `field` in forms and cards, `doc` for whole documents, `composer` for the thread's note box. */
		variant?: 'field' | 'doc' | 'composer';
		/** The height it starts at, in lines; `minHeight` (any CSS length) overrides it. */
		rows?: number;
		minHeight?: string;
		autofocus?: boolean;
		spellcheck?: boolean;
		submitOnEnter?: boolean;
		onsubmit?: () => void;
		/** Escape. */
		oncancel?: () => void;
		oninput?: () => void;
	} = $props();

	let host: HTMLDivElement;
	let view: EditorView | undefined;
	let extensions: Extension;
	/** The text the editor holds, to tell its own changes from changes made to `value` outside. */
	let held = '';
	let focusWhenReady = false;

	onMount(() => {
		held = value;
		extensions = markdownEditor({
			placeholder,
			label,
			spellcheck,
			submitOnEnter,
			tabIndents: variant === 'doc',
			submit: () => onsubmit?.(),
			cancel: () => {
				if (!oncancel) return false;
				oncancel();
				return true;
			},
			changed: (update) => {
				if (!update.docChanged) return;
				held = update.state.doc.toString();
				value = held;
				oninput?.();
			}
		});
		view = new EditorView({ parent: host, state: EditorState.create({ doc: value, extensions }) });
		if (autofocus || focusWhenReady) focus(autofocus);
		return () => view?.destroy();
	});

	// Changed outside (a note sent, a file reloaded or another one opened): start afresh, so undo can't bring back
	// the text that was there before.
	$effect(() => {
		const next = value;
		if (!view || next === held) return;
		held = next;
		const head = Math.min(view.state.selection.main.head, next.length);
		view.setState(EditorState.create({ doc: next, selection: { anchor: head }, extensions }));
	});

	/** Focuses the editor, now or as soon as it exists; `atEnd` puts the cursor after the text. */
	export function focus(atEnd = false) {
		if (!view) {
			focusWhenReady = true;
			return;
		}
		view.focus();
		if (atEnd) view.dispatch({ selection: { anchor: view.state.doc.length }, scrollIntoView: true });
	}
</script>

<div class="md-editor md-{variant}" style:--md-min-height={minHeight ?? `calc(${rows} * 1.55em + 2 * var(--md-pad-y))`} bind:this={host}></div>
