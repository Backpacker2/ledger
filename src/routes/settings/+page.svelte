<script lang="ts">
	import { setTheme, theme, themes } from '$lib/ui.svelte';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();
</script>

<svelte:head><title>Settings · Ledger</title></svelte:head>

<div class="header-wrap flat">
	<div class="header-top">
		<div>
			<h1>Settings</h1>
		</div>
	</div>
</div>

<div class="page">
	<div class="page-inner">
		<div class="panel">
			<h2>Theme</h2>
			<div class="theme-grid">
				{#each themes as t (t.id)}
					<button class="theme-card" class:on={theme.id === t.id} onclick={() => setTheme(t.id)}>
						<span class="theme-swatch">
							{#each t.swatch as colour (colour)}<span style="background:{colour}"></span>{/each}
						</span>
						<span class="theme-name">{t.name}</span>
					</button>
				{/each}
			</div>
		</div>

		<div class="panel">
			<h2>Your data</h2>
			<p class="sub" style="margin:0">
				Threads, posts, documents, user stories and the planboard are stored in one SQLite file on this computer. Back it up by copying it while Ledger is closed.
			</p>
			<dl class="kv">
				<dt>Database</dt>
				<dd class="mono">{data.info.dbPath}</dd>
				<dt>Version</dt>
				<dd>Ledger {data.info.version}</dd>
			</dl>
		</div>
	</div>
</div>
