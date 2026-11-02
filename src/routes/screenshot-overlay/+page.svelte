<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';

	// The capture, stretched over the whole window. The user drags out a rectangle in window units; it is scaled to
	// the capture's own pixels, which differ by the display's scale factor, before it goes to the backend.

	let src = $state('');
	let image = $state<HTMLImageElement>();
	let anchor = $state<{ x: number; y: number } | null>(null);
	let point = $state({ x: 0, y: 0 });
	let width = $state(1);
	let height = $state(1);

	const rect = $derived(
		anchor && {
			x: Math.min(anchor.x, point.x),
			y: Math.min(anchor.y, point.y),
			width: Math.abs(point.x - anchor.x),
			height: Math.abs(point.y - anchor.y)
		}
	);
	const scale = $derived(image ? { x: image.naturalWidth / width, y: image.naturalHeight / height } : { x: 1, y: 1 });

	const cancel = () => void api.screenshotCancel().catch(() => {});

	onMount(() => {
		let url = '';
		api.screenshotImage()
			.then((png) => {
				url = URL.createObjectURL(new Blob([png], { type: 'image/png' }));
				src = url;
			})
			.catch(cancel);
		return () => url && URL.revokeObjectURL(url);
	});

	const inside = (e: PointerEvent) => ({
		x: Math.min(Math.max(e.clientX, 0), width),
		y: Math.min(Math.max(e.clientY, 0), height)
	});

	function down(e: PointerEvent) {
		if (e.button !== 0) return cancel();
		(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
		anchor = point = inside(e);
	}

	function move(e: PointerEvent) {
		point = inside(e);
	}

	async function up(e: PointerEvent) {
		point = inside(e); // the selection ends where the button was released, whatever moved last
		const r = rect;
		anchor = null;
		if (!r || r.width < 4 || r.height < 4) return; // a click, not a drag
		await api
			.screenshotFinish({
				x: Math.round(r.x * scale.x),
				y: Math.round(r.y * scale.y),
				width: Math.max(1, Math.round(r.width * scale.x)),
				height: Math.max(1, Math.round(r.height * scale.y))
			})
			.catch(cancel);
	}
</script>

<svelte:window bind:innerWidth={width} bind:innerHeight={height} onkeydown={(e) => e.key === 'Escape' && cancel()} />

<div
	class="overlay"
	role="presentation"
	onpointerdown={down}
	onpointermove={move}
	onpointerup={up}
	oncontextmenu={(e) => {
		e.preventDefault();
		cancel();
	}}
>
	{#if src}
		<img bind:this={image} {src} alt="" draggable="false" onload={() => api.screenshotOverlayReady()} />
	{/if}
	{#if rect && rect.width > 0 && rect.height > 0}
		<div class="selection" style="left:{rect.x}px;top:{rect.y}px;width:{rect.width}px;height:{rect.height}px">
			<span>{Math.round(rect.width * scale.x)} × {Math.round(rect.height * scale.y)}</span>
		</div>
	{:else}
		<div class="dim"></div>
	{/if}
	<div class="hint-bar">Drag to select an area · Esc to cancel</div>
</div>

<style>
	:global(html),
	:global(body) {
		background: transparent !important;
		overflow: hidden;
		cursor: crosshair;
		user-select: none;
	}

	.overlay {
		position: fixed;
		inset: 0;
		touch-action: none;
	}

	img {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		object-fit: fill;
		pointer-events: none;
	}

	.dim {
		position: absolute;
		inset: 0;
		background: rgba(0, 0, 0, 0.4);
		pointer-events: none;
	}

	/* The rectangle shows the picture undimmed; its enormous shadow dims everything around it. */
	.selection {
		position: absolute;
		border: 1px solid #7c7cf0;
		box-shadow: 0 0 0 100vmax rgba(0, 0, 0, 0.45);
		pointer-events: none;
	}

	.selection span {
		position: absolute;
		left: 0;
		top: calc(100% + 6px);
		padding: 2px 8px;
		border-radius: 6px;
		background: rgba(18, 20, 27, 0.85);
		color: #fff;
		font: 600 12px/1.5 system-ui, sans-serif;
		white-space: nowrap;
	}

	.hint-bar {
		position: absolute;
		top: 18px;
		left: 50%;
		transform: translateX(-50%);
		padding: 6px 14px;
		border-radius: 999px;
		background: rgba(18, 20, 27, 0.85);
		color: #fff;
		font: 500 13px/1.5 system-ui, sans-serif;
		pointer-events: none;
	}
</style>
