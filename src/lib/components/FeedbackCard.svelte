<script lang="ts">
	import { goto } from '$app/navigation';
	import { api } from '$lib/api';
	import { baseName, deliverableUrl, feedbackPromptDraft, threadUrl } from '$lib/actions';
	import { time } from '$lib/format';
	import { attempt, confirmDialog, refresh } from '$lib/ui.svelte';
	import type { Feedback } from '$lib/types';
	import Markdown from './Markdown.svelte';
	import MarkdownEditor from './MarkdownEditor.svelte';
	import PromptModal from './PromptModal.svelte';

	/** `onDeliverable`: shown on the deliverable's own page, where naming the deliverable would only be noise. */
	let {
		project,
		thread,
		feedback,
		onDeliverable = false
	}: { project: string; thread: string; feedback: Feedback; onDeliverable?: boolean } = $props();

	let editing = $state(false);
	let draft = $state('');
	let saving = $state(false);
	let prompting = $state(false);

	const resolved = $derived(feedback.status === 'resolved');

	async function toggle() {
		const next = resolved ? 'open' : 'resolved';
		if (await attempt(() => api.setFeedbackStatus(project, thread, feedback.id, next))) await refresh();
	}

	function startEdit() {
		draft = feedback.body;
		editing = true;
	}

	async function save() {
		if (!draft.trim()) return;
		saving = true;
		const done = await attempt(() => api.updateFeedback(project, thread, feedback.id, draft));
		saving = false;
		if (!done) return;
		editing = false;
		await refresh();
	}

	async function remove() {
		const ok = await confirmDialog({ title: 'Delete this feedback?', confirmLabel: 'Delete', danger: true });
		if (!ok) return;
		if ((await attempt(() => api.deleteFeedback(project, thread, feedback.id))) !== undefined) await refresh();
	}
</script>

<div class="card feedback-card" class:resolved id="f-{feedback.id}">
	<div class="meta">
		<span class="card-kind">{resolved ? 'Feedback · resolved' : 'Feedback'}</span>
		<span class="time">{time(feedback.createdAt)}</span>
		{#if onDeliverable}
			<span class="file">
				{feedback.deliverableVersion ? `on version ${feedback.deliverableVersion}` : 'on the current file'} ·
				<a href="/projects/{project}/threads/{thread}?item=f-{feedback.id}">in its thread</a>
			</span>
		{:else}
			<span class="file">
				on <a href={deliverableUrl(project, feedback.deliverablePath)}>{baseName(feedback.deliverablePath)}</a>
			</span>
		{/if}
	</div>

	{#if editing}
		<MarkdownEditor bind:value={draft} label="Feedback" autofocus onsubmit={save} oncancel={() => (editing = false)} />
		<div class="row-end">
			<span class="hint" style="align-self:center">Ctrl+Enter saves</span>
			<button class="btn small ghost" onclick={() => (editing = false)}>Cancel</button>
			<button class="btn small primary" onclick={save} disabled={saving || !draft.trim()}>Save</button>
		</div>
	{:else}
		<Markdown html={feedback.html} />
		<div class="actions">
			<button class="link accent" onclick={toggle}>{resolved ? 'Reopen' : 'Mark resolved'}</button>
			<button class="link accent" onclick={() => (prompting = true)}>Create prompt…</button>
			<button class="link" onclick={startEdit}>Edit</button>
			<button class="link danger" onclick={remove}>Delete</button>
		</div>
	{/if}
</div>

{#if prompting}
	<!-- The prompt lands in this feedback's own thread. From a deliverable's page that is another screen, so go and show it;
	     in the thread itself the new prompt scrolls into view on its own. -->
	<PromptModal
		{project}
		{thread}
		prompt={null}
		draft={feedbackPromptDraft(feedback)}
		onsaved={onDeliverable ? (saved) => goto(`${threadUrl(project, thread)}?item=pr-${saved.id}`) : undefined}
		onclose={() => (prompting = false)}
	/>
{/if}
