<script lang="ts">
	import SettingsGroup from '$lib/components/settings/SettingsGroup.svelte';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();
</script>

<svelte:head><title>Features · Settings · Ledger</title></svelte:head>

<SettingsGroup group="features" settings={data.settings} />
