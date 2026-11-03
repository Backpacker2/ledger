<script lang="ts">
	import { dashboard, setShowActivity } from '$lib/dashboard.svelte';
</script>

<div class="panel">
	<h2>Dashboard</h2>
	<p class="sub" style="margin:0">What the home page shows besides your projects. Kept on this machine, like the theme; nothing in the vault changes.</p>

	<div class="field">
		<label class="check">
			<input type="checkbox" checked={dashboard.showActivity} onchange={(e) => setShowActivity(e.currentTarget.checked)} />
			Show the contribution graph
		</label>
		<span class="hint">
			A square per day, shaded by how much you made that day: threads, your posts, prompts, feedback, uploads, published versions, Claude Code runs,
			stories and tasks. Filter it by project, and look at the last month, the last twelve months or a year.
		</span>
	</div>
</div>
