<script lang="ts">
	import { newThreadFlow } from '$lib/actions';
	import { date, plural } from '$lib/format';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();
	const recent = $derived(data.threads.slice(0, 6));
</script>

<div class="header-wrap flat">
	<div class="header-top">
		<div>
			<h1>Ledger</h1>
			<div class="sub">Think in threads, draft documents from them, and plan the work on a board.</div>
		</div>
		<div class="header-actions">
			<button class="btn primary" onclick={newThreadFlow}>New thread</button>
		</div>
	</div>
</div>

<div class="page">
	<div class="page-inner">
		{#if data.threads.length === 0}
			<div class="empty-state">
				<h1>Welcome to your ledger</h1>
				<p>
					Start a <strong>thread</strong> for each topic you're juggling and jot markdown notes into it. Later, turn a
					thread's notes into a document, and track the user stories they serve on the planboard.
				</p>
				<button class="btn primary" onclick={newThreadFlow}>Create your first thread</button>
			</div>
		{:else}
			<div class="panel">
				<h2>Recent threads</h2>
				<div class="doc-list">
					{#each recent as t (t.slug)}
						<a class="doc-row" style="grid-template-columns: 1fr 110px 110px 100px" href="/threads/{t.slug}">
							<span class="doc-title">{t.title}</span>
							<span class="doc-date">{plural(t.postCount, 'post')}</span>
							<span class="doc-date">{plural(t.documentCount, 'document')}</span>
							<span class="doc-date">{date(t.lastActivity)}</span>
						</a>
					{/each}
				</div>
			</div>
		{/if}

		<div class="tiles">
			<a class="tile" href="/planboard"><b>Planboard</b><span>User stories across Backlog → Drafting → Documented.</span></a>
			<a class="tile" href="/documents"><b>Documents</b><span>Drafts and published versions, from every thread.</span></a>
			<a class="tile" href="/document-types"><b>Document types</b><span>Guiding questions that seed a thread when you start drafting.</span></a>
		</div>
	</div>
</div>
