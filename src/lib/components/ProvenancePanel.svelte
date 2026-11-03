<script lang="ts">
	import { api } from '$lib/api';
	import { deliverableUrl, formatSize, threadUrl } from '$lib/actions';
	import { ago, date, dateTime, dayKey, plural } from '$lib/format';
	import { MMM, authorLabel, sizeChange } from '$lib/mmm';
	import { attempt, refresh } from '$lib/ui.svelte';
	import type { Deliverable, MmmLevel, Provenance, ProvenanceVersion, VersionUpdate } from '$lib/types';
	import MmmBadge from './MmmBadge.svelte';

	let {
		project,
		doc,
		provenance,
		onclose
	}: { project: string; doc: Deliverable; provenance: Provenance; onclose: () => void } = $props();

	const summary = $derived(provenance.summary);
	const latest = $derived(doc.versions.at(-1) ?? null);

	let busy = $state(false);

	async function update(v: ProvenanceVersion, change: VersionUpdate) {
		busy = true;
		const done = await attempt(() => api.updateVersionRecord(project, doc.basePath, v.version, change));
		busy = false;
		if (done) await refresh();
	}

	const setLevel = (v: ProvenanceVersion, value: string) => update(v, { mmm: value as MmmLevel | '' });

	/** "3 Oct", or "3 Oct – 9 Oct" when a thread's part in the file spans days. */
	function span(first: string, last: string): string {
		return dayKey(first) === dayKey(last) ? date(first) : `${date(first)} – ${date(last)}`;
	}

	const promptUrl = (thread: string, id: string) => `${threadUrl(project, thread)}?item=pr-${id}`;
</script>

<div class="pv-head">
	<h2>Produced by</h2>
	<button class="icon-btn" aria-label="Close" onclick={onclose}>✕</button>
</div>

<div class="pv-summary">
	<div class="pv-line">
		{#if summary.label}
			<MmmBadge level={summary.label.level} tagline />
			<span class="pv-meta">
				as declared for v{summary.label.version}{summary.label.version !== latest ? `, latest is v${latest}` : ''}
			</span>
		{:else}
			<span class="pv-meta">No AI-use label declared yet.</span>
		{/if}
	</div>
	<div class="pv-counts">
		{plural(summary.versions, 'version')}
		· {summary.ai} made with AI · {summary.human} by you{summary.unrecorded ? ` · ${summary.unrecorded} not recorded` : ''}
	</div>
	<div class="pv-hint">
		Claude Code runs started here are recorded as they happen. AI used elsewhere is only as complete as the box you tick when you publish.
	</div>
</div>

{#if provenance.draft}
	<div class="pv-draft">
		<strong>Draft{provenance.draft.since ? ` after v${provenance.draft.since}` : ''}</strong>
		<span class="pv-meta">the live file changed {ago(provenance.draft.modified)}</span>
		{#if provenance.draft.stoppedRun}
			<div class="pv-warn">
				Claude Code’s run of “{provenance.draft.stoppedRun.promptTitle}” changed this file{provenance.draft.stoppedRun.status === 'finished'
					? ''
					: ` and was ${provenance.draft.stoppedRun.status}`}, and no version holds it yet. Publishing starts with “made with AI” ticked.
			</div>
		{/if}
	</div>
{/if}

{#each provenance.groups as group (group.thread?.slug ?? '—')}
	<section class="pv-group">
		<div class="pv-thread">
			{#if group.thread}
				{#if group.thread.exists}
					<a href={threadUrl(project, group.thread.slug)} title="Open the thread">{group.thread.title}</a>
				{:else}
					<span class="name" title={group.thread.slug}>{group.thread.title}</span>
				{/if}
			{:else}
				<span class="name">Not in any thread</span>
			{/if}
			<span class="pv-meta">{span(group.firstAt, group.lastAt)}</span>
		</div>
		{#if !group.thread}
			<div class="pv-hint">Published without a thread, or before Ledger kept track of where.</div>
		{/if}

		<div class="pv-entries">
			{#each group.entries as e (e.kind + ':' + (e.kind === 'version' ? e.version : e.id))}
				{#if e.kind === 'prompt'}
					<div class="pv-row">
						<span class="glyph" title="Prompt">◇</span>
						<span>Asked in</span>
						{#if group.thread?.exists}
							<a href={promptUrl(group.thread.slug, e.id)}>{e.title}</a>
						{:else}
							<span>{e.title}</span>
						{/if}
					</div>
				{:else if e.kind === 'run'}
					<div class="pv-row">
						<span class="glyph" title="Claude Code run">▶</span>
						<span>
							Claude Code run{e.status === 'finished' ? '' : `, ${e.status}`}
							· {e.model || 'default model'} · {e.seconds < 60 ? `${e.seconds} s` : `${Math.floor(e.seconds / 60)} min ${e.seconds % 60} s`}
							{e.version ? (e.status === 'finished' ? `· published v${e.version}` : `· its changes went into v${e.version}`) : '· published nothing'}
						</span>
					</div>
				{:else if e.kind === 'feedback'}
					<div class="pv-row">
						<span class="glyph" title="Feedback">✎</span>
						<a href="{threadUrl(project, group.thread?.slug ?? '')}?item=f-{e.id}">
							Feedback{e.status === 'open' ? ' (open)' : ' (resolved)'}{e.version ? ` on v${e.version}` : ''}: {e.excerpt}
						</a>
					</div>
				{:else}
					<div class="pv-version" class:current={doc.version === e.version}>
						<div class="pv-line">
							<a class="vchip" class:on={doc.version === e.version} href={deliverableUrl(project, e.path)}>v{e.version}</a>
							<span class="pv-by" class:ai={e.by === 'ai'} class:unknown={e.by === 'unknown'}>{authorLabel(e)}</span>
							{#if e.mmm}
								<MmmBadge level={e.mmm} />
							{:else}
								<select class="pv-level" aria-label="Set the AI-use label of v{e.version}" disabled={busy} value="" onchange={(ev) => setLevel(e, ev.currentTarget.value)}>
									<option value="">Set label…</option>
									{#each MMM as m (m.level)}<option value={m.level}>{m.name} — {m.tagline}</option>{/each}
								</select>
							{/if}
						</div>
						<div class="pv-line">
							<span class="pv-when">{dateTime(e.at)}</span>
							{#if e.sizeChange}<span class="pv-size">{sizeChange(e.sizeChange)}</span>{/if}
						</div>
						{#if e.note}<p class="pv-note">{e.note}</p>{/if}
						{#if e.prompt}
							<div class="pv-meta">
								from the prompt
								{#if e.prompt.exists}
									<a href={promptUrl(e.prompt.threadSlug, e.prompt.id)}>{e.prompt.title}</a>
								{:else}
									<span>{e.prompt.title}</span>
								{/if}
							</div>
						{/if}
						<details class="pv-details">
							<summary>Details</summary>
							<dl class="kv">
								<dt>File</dt>
								<dd class="mono">{e.path}</dd>
								<dt>Size</dt>
								<dd>{formatSize(e.size)}</dd>
								<dt>SHA-256</dt>
								<dd class="mono">{e.sha256 ?? 'not worked out for this kind of file'}</dd>
								<dt>Published</dt>
								<dd>{dateTime(e.at)}</dd>
								<dt>Record</dt>
								<dd class="mono">{e.record ? `versions/${e.record}.md` : 'none, this version is older than records'}</dd>
								{#if e.post}
									<dt>Note</dt>
									<dd><a href="{threadUrl(project, e.post.threadSlug)}?item=p-{e.post.id}">in its thread</a></dd>
								{/if}
								{#if e.prompt}
									<dt>Prompt</dt>
									<dd class="mono">{e.prompt.id}</dd>
								{/if}
								{#if e.run}
									<dt>Run</dt>
									<dd class="mono">{e.run.id}</dd>
									<dt>Model</dt>
									<dd>{e.run.model || 'default'}</dd>
									<dt>Mode</dt>
									<dd>{e.run.permissionMode || 'default'}</dd>
									<dt>Run</dt>
									<dd>{e.run.status} · {e.run.seconds} s{e.run.turns !== null ? ` · ${plural(e.run.turns, 'turn')}` : ''}</dd>
								{/if}
							</dl>
							<div class="pv-edit">
								{#if !e.run}
									<span class="pv-meta">Made by</span>
									<button class="btn small" class:primary={e.by === 'human'} disabled={busy} onclick={() => update(e, { by: 'human' })}>You</button>
									<button class="btn small" class:primary={e.by === 'ai'} disabled={busy} onclick={() => update(e, { by: 'ai' })}>AI</button>
								{/if}
								{#if e.mmm}
									<select class="pv-level" aria-label="Change the AI-use label of v{e.version}" disabled={busy} value={e.mmm} onchange={(ev) => setLevel(e, ev.currentTarget.value)}>
										<option value="">Remove the label</option>
										{#each MMM as m (m.level)}<option value={m.level}>{m.name} — {m.tagline}</option>{/each}
									</select>
								{/if}
							</div>
						</details>
					</div>
				{/if}
			{/each}
		</div>
	</section>
{:else}
	{#if !provenance.draft}
		<div class="pv-hint">Nothing has produced this file yet. Versions you publish, and the prompts and runs behind them, are listed here by thread.</div>
	{/if}
{/each}
