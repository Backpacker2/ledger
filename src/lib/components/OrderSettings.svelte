<script lang="ts">
	import { listOrder, setExplorerOrder, setFeedbackOrder, setPostOrder } from '$lib/order.svelte';
	import type { Order } from '$lib/order';

	const choices: { value: Order; label: string }[] = [
		{ value: 'asc', label: 'Oldest first' },
		{ value: 'desc', label: 'Newest first' }
	];
</script>

<div class="panel">
	<h2>Order</h2>
	<p class="sub" style="margin:0">Which way lists are read. This only changes how they are shown; nothing in the vault changes.</p>

	<div class="field">
		Posts in a thread
		<div class="seg" role="group" aria-label="Order of posts in a thread" style="align-self:flex-start">
			{#each choices as c (c.value)}
				<button class:on={listOrder.posts === c.value} aria-pressed={listOrder.posts === c.value} onclick={() => setPostOrder(c.value)}>
					{c.label}
				</button>
			{/each}
		</div>
		<span class="hint">
			Inside a thread: its notes, prompts, feedback and uploads, read together. Oldest first opens at the bottom, newest first at the top.
		</span>
	</div>

	<div class="field">
		Feedback on a deliverable
		<div class="seg" role="group" aria-label="Order of feedback on a deliverable" style="align-self:flex-start">
			{#each choices as c (c.value)}
				<button class:on={listOrder.feedback === c.value} aria-pressed={listOrder.feedback === c.value} onclick={() => setFeedbackOrder(c.value)}>
					{c.label}
				</button>
			{/each}
		</div>
		<span class="hint">The feedback list on a deliverable’s own page. Inside a thread, feedback follows the posts setting above.</span>
	</div>

	<div class="field">
		Threads in the explorer
		<div class="seg" role="group" aria-label="Order of threads in the explorer" style="align-self:flex-start">
			{#each choices as c (c.value)}
				<button class:on={listOrder.explorer === c.value} aria-pressed={listOrder.explorer === c.value} onclick={() => setExplorerOrder(c.value)}>
					{c.label}
				</button>
			{/each}
		</div>
		<span class="hint">
			The list of threads in the sidebar and on a project’s page, by latest activity. It doesn’t change the posts inside a thread.
		</span>
	</div>
</div>
