<script lang="ts">
	import { listOrder, setOrder } from '$lib/order.svelte';
	import type { Order } from '$lib/order';

	const choices: { value: Order; label: string }[] = [
		{ value: 'desc', label: 'Newest first' },
		{ value: 'asc', label: 'Oldest first' }
	];
</script>

<div class="panel">
	<h2>Order</h2>
	<p class="sub" style="margin:0">
		Which way lists are read. One choice for all of them, newest first unless you change it. This only changes how things are shown; nothing in the
		vault changes.
	</p>

	<div class="field">
		<div class="seg" role="group" aria-label="Order of lists" style="align-self:flex-start">
			{#each choices as c (c.value)}
				<button class:on={listOrder.order === c.value} aria-pressed={listOrder.order === c.value} onclick={() => setOrder(c.value)}>
					{c.label}
				</button>
			{/each}
		</div>
		<span class="hint">
			It applies to the posts in a thread (its notes, prompts, feedback and uploads, read together; newest first opens at the top, oldest first at the
			bottom), the threads in the sidebar and on a project’s page (by latest activity), the feedback on a deliverable, a deliverable’s versions, and its
			“Produced by” window, where the threads and what happened in each turn over together.
		</span>
	</div>
</div>
