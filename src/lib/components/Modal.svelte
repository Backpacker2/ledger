<script module lang="ts">
	// Open modals, innermost last: Escape only closes the one on top.
	const stack: object[] = [];
</script>

<script lang="ts">
	import X from '@lucide/svelte/icons/x';
	import type { Snippet } from 'svelte';

	let {
		title,
		onclose,
		wide = false,
		children,
		footer
	}: { title: string; onclose: () => void; wide?: boolean; children: Snippet; footer?: Snippet } = $props();

	const me = {};
	let root: HTMLDivElement;

	$effect(() => {
		stack.push(me);
		root.querySelector<HTMLElement>('[data-autofocus], input:not([type=checkbox]), select, .cm-content')?.focus();
		return () => {
			stack.splice(stack.indexOf(me), 1);
		};
	});

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Escape' && stack.at(-1) === me) {
			e.preventDefault();
			onclose();
		}
	}
</script>

<svelte:window {onkeydown} />

<div class="modal-scrim" role="presentation" onmousedown={(e) => e.target === e.currentTarget && onclose()}>
	<div class="modal" class:wide role="dialog" aria-modal="true" aria-label={title} bind:this={root}>
		<div class="modal-head">
			<h2>{title}</h2>
			<button class="icon-btn" aria-label="Close" onclick={onclose}><X size={18} /></button>
		</div>
		<div class="modal-body">{@render children()}</div>
		{#if footer}<div class="modal-foot">{@render footer()}</div>{/if}
	</div>
</div>
