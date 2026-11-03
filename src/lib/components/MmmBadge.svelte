<script lang="ts">
	import { mmmInfo } from '$lib/mmm';
	import type { MmmLevel } from '$lib/types';

	let { level, tagline = false }: { level: MmmLevel | string; tagline?: boolean } = $props();

	const info = $derived(mmmInfo(level));
</script>

{#if info}
	<span class="mmm" title="{info.name} — {info.tagline}. {info.meaning} (Me & My Machine label, mmmlabel.tech)" style="--c: {info.color}">
		<span class="mmm-dot"></span>{info.name}{#if tagline}<span class="mmm-tag">{info.tagline}</span>{/if}
	</span>
{/if}
