<script lang="ts">
	import CircleArrowUp from '@lucide/svelte/icons/circle-arrow-up';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import X from '@lucide/svelte/icons/x';
	import Markdown from './Markdown.svelte';
	import { date } from '$lib/format';
	import { installUpdate, later, stopWaiting, updates } from '$lib/updates.svelte';

	const info = $derived(updates.available);
	const busy = $derived(updates.phase === 'working' || updates.phase === 'waiting');
	const percent = $derived.by(() => {
		const p = updates.progress;
		return p?.total ? Math.min(100, Math.round((p.downloaded / p.total) * 100)) : null;
	});
	const downloading = $derived(updates.phase === 'working' && updates.progress !== null && percent !== 100);

	/** “A”, “B” and “C”. */
	function quoted(titles: string[]) {
		const q = titles.map((t) => `“${t.trim() || 'a prompt'}”`);
		return q.length > 1 ? `${q.slice(0, -1).join(', ')} and ${q.at(-1)}` : (q[0] ?? '');
	}
</script>

{#if info && !updates.hidden}
	<aside class="update-notice" class:major={info.major} aria-label="Update available">
		<div class="un-head">
			<span class="un-icon"><CircleArrowUp size={18} /></span>
			<div class="un-title">
				<strong>Ledger v{info.version} is available</strong>
				<span class="un-sub">
					You have v{info.currentVersion}{info.date ? ` · released ${date(info.date)}` : ''}{info.simulated ? ' · simulated' : ''}
				</span>
			</div>
			{#if !busy}
				<button class="icon-btn" aria-label="Later" title="Later" onclick={later}><X size={16} /></button>
			{/if}
		</div>

		{#if info.major}
			<div class="un-major">
				<TriangleAlert size={15} />
				<span>
					<b>A major update.</b> Something you rely on may work differently afterwards{info.breakingHtml
						? ': read what changes before you install.'
						: '.'}
				</span>
			</div>
		{/if}

		<div class="un-body">
			{#if info.breakingHtml}
				<div class="un-breaking">
					<div class="un-label">Breaking changes</div>
					<Markdown html={info.breakingHtml} />
				</div>
			{/if}
			{#if info.notesHtml}
				<div class="un-notes"><Markdown html={info.notesHtml} /></div>
			{:else if !info.breakingHtml}
				<div class="hint">No release notes.</div>
			{/if}
		</div>

		{#if updates.phase === 'idle'}
			<div class="un-foot">
				<button class="btn primary" onclick={() => installUpdate()}>Install and restart</button>
				<button class="btn ghost" onclick={later}>Later</button>
			</div>
		{:else if updates.phase === 'working'}
			<div class="un-status">
				{#if downloading}
					<div class="un-progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent ?? undefined}>
						<span style:width="{percent ?? 0}%"></span>
					</div>
					<span>Downloading… {percent ?? 0}%</span>
				{:else}
					<span><span class="dot pulse"></span> Getting the update ready. Ledger restarts when it is installed.</span>
				{/if}
			</div>
		{:else if updates.phase === 'waiting'}
			<div class="un-status">
				<span><span class="dot warn pulse"></span> Downloaded. Waiting for {updates.waitingFor.join(' and ')}, then Ledger installs it and restarts.</span>
				<div class="un-foot"><button class="btn small ghost" onclick={stopWaiting}>Don’t install now</button></div>
			</div>
		{:else if updates.phase === 'ask'}
			<div class="un-status">
				<span>
					Claude Code is still running {quoted(updates.runs)}. Restarting stops {updates.runs.length === 1 ? 'that run' : 'those runs'}.
				</span>
				<div class="un-foot">
					<button class="btn primary" onclick={() => installUpdate('wait')}>Wait, then install</button>
					<button class="btn danger" onclick={() => installUpdate('stop')}>Stop and install</button>
					<button class="btn ghost" onclick={stopWaiting}>Cancel</button>
				</div>
			</div>
		{:else if updates.phase === 'simulated'}
			<div class="un-status">
				<div class="notice ok">
					Simulated: a release build would now install v{info.version} and restart. It waited for what was going on, as a real update does; nothing was
					installed.
				</div>
				<div class="un-foot"><button class="btn small" onclick={later}>Close</button></div>
			</div>
		{:else}
			<div class="un-status">
				<div class="notice bad">{updates.error}</div>
				<div class="un-foot">
					<button class="btn primary" onclick={() => installUpdate()}>Try again</button>
					<button class="btn ghost" onclick={later}>Later</button>
				</div>
			</div>
		{/if}
	</aside>
{/if}
