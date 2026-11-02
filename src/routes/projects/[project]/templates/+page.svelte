<script lang="ts">
	import TemplatesPanel from '$lib/components/TemplatesPanel.svelte';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();
</script>

<svelte:head><title>Templates · {data.project.name} · Ledger</title></svelte:head>

<div class="page">
	<div class="page-inner">
		<TemplatesPanel scope={data.project.slug} templates={data.templates} />
	</div>
</div>
