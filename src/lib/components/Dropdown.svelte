<script lang="ts" module>
	export interface DropdownOption {
		value: string;
		label: string;
		/** Options that share a group are listed under its name; a new name starts a new section. */
		group?: string;
	}
</script>

<script lang="ts">
	import Check from '@lucide/svelte/icons/check';
	import ChevronDown from '@lucide/svelte/icons/chevron-down';
	import { tick } from 'svelte';

	// A select that looks like the rest of the app: a button that opens a list. Keyboard: Enter, Space or the arrows
	// open it; the arrows, Home and End move; Enter or Space picks; Escape or Tab closes. Typing a letter jumps to the
	// next option that starts with it.

	let {
		value = $bindable(),
		options,
		label,
		align = 'start'
	}: { value: string; options: DropdownOption[]; label: string; align?: 'start' | 'end' } = $props();

	const id = `dd-${Math.random().toString(36).slice(2, 9)}`;
	let open = $state(false);
	let active = $state(0);
	let root: HTMLElement | undefined = $state();
	let trigger: HTMLButtonElement | undefined = $state();
	let list: HTMLElement | undefined = $state();

	const current = $derived(options.find((o) => o.value === value) ?? options[0]);

	function show() {
		active = Math.max(0, options.findIndex((o) => o.value === value));
		open = true;
		void tick().then(() => {
			list?.focus();
			list?.querySelector(`#${id}-${active}`)?.scrollIntoView({ block: 'nearest' });
		});
	}

	function close(refocus = true) {
		open = false;
		if (refocus) trigger?.focus();
	}

	function pick(i: number) {
		value = options[i].value;
		close();
	}

	function move(to: number) {
		active = Math.min(options.length - 1, Math.max(0, to));
		list?.querySelector(`#${id}-${active}`)?.scrollIntoView({ block: 'nearest' });
	}

	function onTriggerKey(e: KeyboardEvent) {
		if (['ArrowDown', 'ArrowUp', 'Enter', ' '].includes(e.key)) {
			e.preventDefault();
			show();
		}
	}

	function onListKey(e: KeyboardEvent) {
		const keys: Record<string, () => void> = {
			ArrowDown: () => move(active + 1),
			ArrowUp: () => move(active - 1),
			Home: () => move(0),
			End: () => move(options.length - 1),
			Enter: () => pick(active),
			' ': () => pick(active),
			Escape: () => close()
		};
		if (keys[e.key]) {
			e.preventDefault();
			keys[e.key]();
		} else if (e.key === 'Tab') {
			close(false);
		} else if (e.key.length === 1) {
			const letter = e.key.toLowerCase();
			const order = [...options.keys()].map((k) => (active + 1 + k) % options.length);
			const next = order.find((k) => options[k].label.toLowerCase().startsWith(letter));
			if (next !== undefined) move(next);
		}
	}

	// A click anywhere else closes it.
	$effect(() => {
		if (!open) return;
		const away = (e: PointerEvent) => {
			if (root && !root.contains(e.target as Node)) close(false);
		};
		document.addEventListener('pointerdown', away);
		return () => document.removeEventListener('pointerdown', away);
	});
</script>

<div class="dd" bind:this={root}>
	<button
		bind:this={trigger}
		type="button"
		class="dd-trigger"
		class:open
		aria-haspopup="listbox"
		aria-expanded={open}
		aria-label="{label}: {current?.label}"
		onclick={() => (open ? close() : show())}
		onkeydown={onTriggerKey}
	>
		<span class="dd-value">{current?.label}</span>
		<ChevronDown size={14} class="dd-chevron" />
	</button>

	{#if open}
		<div
			bind:this={list}
			class="dd-menu"
			class:end={align === 'end'}
			role="listbox"
			tabindex="-1"
			aria-label={label}
			aria-activedescendant="{id}-{active}"
			onkeydown={onListKey}
		>
			{#each options as o, i (o.value)}
				{#if o.group && o.group !== options[i - 1]?.group}
					<div class="dd-group" role="presentation">{o.group}</div>
				{/if}
				<!-- svelte-ignore a11y_click_events_have_key_events -->
				<div
					id="{id}-{i}"
					class="dd-option"
					class:active={i === active}
					class:selected={o.value === value}
					role="option"
					aria-selected={o.value === value}
					tabindex="-1"
					onpointerenter={() => (active = i)}
					onclick={() => pick(i)}
				>
					<span class="dd-label">{o.label}</span>
					{#if o.value === value}<Check size={14} />{/if}
				</div>
			{/each}
		</div>
	{/if}
</div>
