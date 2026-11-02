<script lang="ts">
	import Modal from './Modal.svelte';
	import { dialog } from '$lib/ui.svelte';

	let value = $state('');

	$effect(() => {
		const d = dialog.current;
		if (d?.kind === 'prompt') value = d.initial ?? '';
	});

	function finish(ok: boolean) {
		const d = dialog.current;
		if (!d) return;
		dialog.current = null;
		if (d.kind === 'confirm') d.resolve(ok);
		else d.resolve(ok && value.trim() ? value.trim() : null);
	}
</script>

{#if dialog.current}
	{@const d = dialog.current}
	<Modal title={d.title} onclose={() => finish(false)}>
		{#if d.kind === 'confirm'}
			{#if d.message}<p style="margin:0;color:var(--muted)">{d.message}</p>{/if}
		{:else}
			<label class="field">
				{d.label ?? ''}
				<input
					bind:value
					placeholder={d.placeholder ?? ''}
					onkeydown={(e) => e.key === 'Enter' && !e.isComposing && finish(true)}
				/>
			</label>
		{/if}
		{#snippet footer()}
			<span class="spacer-h"></span>
			<button class="btn ghost" onclick={() => finish(false)}>Cancel</button>
			<button
				class="btn {d.kind === 'confirm' && d.danger ? 'danger' : 'primary'}"
				data-autofocus={d.kind === 'confirm' ? '' : undefined}
				onclick={() => finish(true)}
			>
				{d.confirmLabel ?? (d.kind === 'confirm' ? 'Confirm' : 'OK')}
			</button>
		{/snippet}
	</Modal>
{/if}
