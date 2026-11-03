<script lang="ts">
	import SettingsGroup from '$lib/components/settings/SettingsGroup.svelte';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();
</script>

<svelte:head><title>Logs · Settings · Ledger</title></svelte:head>

<SettingsGroup group="logs" settings={data.settings} />
