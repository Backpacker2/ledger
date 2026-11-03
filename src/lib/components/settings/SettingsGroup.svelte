<script lang="ts">
	// The panels of one settings group, as its tab shows them.
	import { panels, type GroupId } from './panels';
	import type { Settings } from '$lib/types';

	let { group, settings }: { group: GroupId; settings: Settings } = $props();
</script>

{#each panels.filter((p) => p.group === group) as p (p.id)}
	<p.component {settings} />
{/each}
