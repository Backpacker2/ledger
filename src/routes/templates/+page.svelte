<script lang="ts">
	import TemplatesPanel from '$lib/components/TemplatesPanel.svelte';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();
</script>

<svelte:head><title>Templates · Ledger</title></svelte:head>

<div class="header-wrap flat">
	<div class="header-top">
		<div>
			<h1>Templates</h1>
			<div class="sub">
				Shared by every project. A prompt started from a template gets its guiding questions as posts to answer, and its deliverable starts
				from the template’s skeleton. A project can replace any of them with its own.
			</div>
		</div>
	</div>
</div>

<div class="page">
	<div class="page-inner">
		<TemplatesPanel scope="" templates={data.templates} />
	</div>
</div>
