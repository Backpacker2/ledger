<script lang="ts">
	import { api } from '$lib/api';
	import { mimeOf } from '$lib/actions';

	/** An image from a project's folder, read through the backend (the webview can't open vault files itself). */
	let { project, path, alt = '', cls = 'file-thumb' }: { project: string; path: string; alt?: string; cls?: string } = $props();

	let src = $state<string | null>(null);
	let failed = $state(false);

	// Derived, so the picture is only fetched again when the file really is another one — not each time a
	// refresh hands this component a new (equal) object to read the same path from.
	const currentProject = $derived(project);
	const currentPath = $derived(path);

	$effect(() => {
		const p = currentProject;
		const f = currentPath;
		let url: string | null = null;
		let stale = false;
		src = null;
		failed = false;
		api.readProjectFile(p, f)
			.then((bytes) => {
				if (stale) return;
				url = URL.createObjectURL(new Blob([bytes], { type: mimeOf(f) }));
				src = url;
			})
			.catch(() => {
				if (!stale) failed = true;
			});
		return () => {
			stale = true;
			// A moment later, not now: the <img> may still be on screen for the rest of this update.
			const old = url;
			if (old) setTimeout(() => URL.revokeObjectURL(old), 2000);
		};
	});
</script>

{#if src}
	<img class={cls} {src} {alt} />
{:else if failed}
	<span class="missing">Couldn't load {alt || path}</span>
{/if}
