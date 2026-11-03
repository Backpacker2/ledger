<script lang="ts">
	import SettingsGroup from '$lib/components/settings/SettingsGroup.svelte';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();
</script>

<svelte:head><title>About · Settings · Ledger</title></svelte:head>

<SettingsGroup group="about" settings={data.settings} />
