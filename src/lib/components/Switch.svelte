<script lang="ts">
	// An on/off switch. It shows `checked` and asks for the other state with `onchange`; the caller decides
	// (after a confirmation, say) and passes the new state back in, so a cancelled change never shows as made.
	let {
		checked,
		label,
		disabled = false,
		onchange
	}: { checked: boolean; label: string; disabled?: boolean; onchange: (next: boolean) => void } = $props();
</script>

<button type="button" role="switch" class="switch" class:on={checked} aria-checked={checked} aria-label={label} title={label} {disabled} onclick={() => onchange(!checked)}>
	<span class="knob"></span>
</button>

<style>
	.switch {
		position: relative;
		flex-shrink: 0;
		width: 38px;
		height: 22px;
		padding: 0;
		border: 1px solid var(--line);
		border-radius: 999px;
		background: var(--card-2);
		transition: background 0.15s, border-color 0.15s;
	}
	.switch.on {
		background: var(--accent);
		border-color: var(--accent);
	}
	.switch:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
	.switch:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}
	.knob {
		position: absolute;
		top: 2px;
		left: 2px;
		width: 16px;
		height: 16px;
		border-radius: 50%;
		background: var(--muted);
		transition: transform 0.15s, background 0.15s;
	}
	.switch.on .knob {
		transform: translateX(16px);
		background: var(--accent-ink);
	}
</style>
