<script lang="ts">
	import Switch from '$lib/components/Switch.svelte';
	import { dashboard, setShowActivity } from '$lib/dashboard.svelte';
</script>

<div class="panel">
	<div class="panel-head">
		<Switch checked={dashboard.showActivity} label="Show the contribution graph" onchange={setShowActivity} />
		<h2>Contribution graph</h2>
	</div>
	<p class="sub" style="margin:0">
		Shown on the home page, under your projects: a square per day, shaded by how much you made that day: threads, your posts, prompts, feedback,
		uploads, published versions, Claude Code runs, stories and tasks. Filter it by project, and look at the last month, the last twelve months or a
		year. Kept on this machine, like the theme; nothing in the vault changes.
	</p>
</div>
