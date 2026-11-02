<script lang="ts">
	import { toasts } from '$lib/ui.svelte';
</script>

<div class="toasts" aria-live="polite">
	{#each toasts as t (t.id)}
		<div class="toast" class:error={t.error} role={t.error ? 'alert' : 'status'}>{t.message}</div>
	{/each}
</div>
