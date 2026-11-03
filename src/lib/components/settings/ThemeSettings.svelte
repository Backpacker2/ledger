<script lang="ts">
	// Picking the colour theme. Kept on this machine; nothing in the vault changes.
	import { setTheme, theme, themes } from '$lib/ui.svelte';
</script>

<div class="panel">
	<h2>Theme</h2>
	<div class="theme-grid">
		{#each themes as t (t.id)}
			<button class="theme-card" class:on={theme.id === t.id} onclick={() => setTheme(t.id)}>
				<span class="theme-swatch" data-theme={t.id}>
					<span class="side"></span><span class="panel"></span><span class="bg"></span><span class="accent"></span>
				</span>
				<span class="theme-name">{t.name}</span>
			</button>
		{/each}
	</div>
</div>
