<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { Go2rtcPlayer } from '$lib/player';

	const COLS = 80;
	const ROWS = 45;
	const CELL = 8;
	const W = COLS * CELL; // 640
	const H = ROWS * CELL; // 360

	let videoEl = $state<HTMLVideoElement | undefined>(undefined);
	let canvas: HTMLCanvasElement;
	let ctx: CanvasRenderingContext2D;
	let mask = $state(new Uint8Array(COLS * ROWS));
	let painting = $state(false);
	let brushValue = $state(1); // 1 = mask, 0 = unmask
	let brushSize = $state(1); // radius in cells (1 = single cell, 2 = 3x3, etc.)
	let dirty = $state(false);
	let saving = $state(false);
	let statusMsg = $state('');

	let player = $state<Go2rtcPlayer | null>(null);

	$effect(() => {
		if (!videoEl || !player) return;
		player.mount(videoEl);
	});

	onMount(async () => {
		ctx = canvas.getContext('2d')!;

		player = new Go2rtcPlayer('main');

		await loadMask();
		draw();
	});

	onDestroy(() => {
		player?.unmount();
	});

	async function loadMask() {
		try {
			const res = await fetch('/api/mask');
			if (res.ok) {
				const buf = await res.arrayBuffer();
				if (buf.byteLength === COLS * ROWS) {
					mask = new Uint8Array(buf);
				}
			}
		} catch (e) {
			console.warn('Failed to load mask:', e);
		}
	}

	async function saveMask() {
		saving = true;
		statusMsg = '';
		try {
			const res = await fetch('/api/mask', {
				method: 'PUT',
				headers: { 'Content-Type': 'application/octet-stream' },
				body: mask,
			});
			if (res.ok) {
				dirty = false;
				statusMsg = 'Saved';
				setTimeout(() => { if (statusMsg === 'Saved') statusMsg = ''; }, 2000);
			} else {
				statusMsg = `Error: ${res.status}`;
			}
		} catch (e) {
			statusMsg = `Error: ${e}`;
		}
		saving = false;
	}

	function clearMask() {
		mask = new Uint8Array(COLS * ROWS);
		dirty = true;
		draw();
	}

	function fillMask() {
		mask = new Uint8Array(COLS * ROWS).fill(1);
		dirty = true;
		draw();
	}

	function cellFromEvent(e: MouseEvent | TouchEvent): [number, number] | null {
		const rect = canvas.getBoundingClientRect();
		const scaleX = W / rect.width;
		const scaleY = H / rect.height;
		let clientX: number, clientY: number;
		if ('touches' in e) {
			if (e.touches.length === 0) return null;
			clientX = e.touches[0].clientX;
			clientY = e.touches[0].clientY;
		} else {
			clientX = e.clientX;
			clientY = e.clientY;
		}
		const x = Math.floor((clientX - rect.left) * scaleX / CELL);
		const y = Math.floor((clientY - rect.top) * scaleY / CELL);
		if (x < 0 || x >= COLS || y < 0 || y >= ROWS) return null;
		return [x, y];
	}

	function paintBrush(cx: number, cy: number) {
		const r = brushSize - 1;
		let changed = false;
		for (let dy = -r; dy <= r; dy++) {
			for (let dx = -r; dx <= r; dx++) {
				const x = cx + dx;
				const y = cy + dy;
				if (x < 0 || x >= COLS || y < 0 || y >= ROWS) continue;
				const idx = y * COLS + x;
				if (mask[idx] !== brushValue) {
					mask[idx] = brushValue;
					changed = true;
				}
			}
		}
		if (changed) {
			dirty = true;
			draw();
		}
	}

	function onPointerDown(e: MouseEvent) {
		painting = true;
		const cell = cellFromEvent(e);
		if (!cell) return;
		brushValue = (e.button === 2 || e.ctrlKey) ? 0 : 1;
		paintBrush(cell[0], cell[1]);
	}

	function onPointerMove(e: MouseEvent) {
		if (!painting) return;
		const cell = cellFromEvent(e);
		if (cell) paintBrush(cell[0], cell[1]);
	}

	function onPointerUp() {
		painting = false;
	}

	function onTouchStart(e: TouchEvent) {
		e.preventDefault();
		painting = true;
		brushValue = 1;
		const cell = cellFromEvent(e);
		if (cell) paintBrush(cell[0], cell[1]);
	}

	function onTouchMove(e: TouchEvent) {
		e.preventDefault();
		if (!painting) return;
		const cell = cellFromEvent(e);
		if (cell) paintBrush(cell[0], cell[1]);
	}

	function onTouchEnd(e: TouchEvent) {
		e.preventDefault();
		painting = false;
	}

	function draw() {
		if (!ctx) return;
		ctx.clearRect(0, 0, W, H);
		// Grid lines
		ctx.strokeStyle = 'rgba(255,255,255,0.06)';
		ctx.lineWidth = 0.5;
		for (let x = 0; x <= COLS; x++) {
			ctx.beginPath();
			ctx.moveTo(x * CELL, 0);
			ctx.lineTo(x * CELL, H);
			ctx.stroke();
		}
		for (let y = 0; y <= ROWS; y++) {
			ctx.beginPath();
			ctx.moveTo(0, y * CELL);
			ctx.lineTo(W, y * CELL);
			ctx.stroke();
		}
		// Masked cells
		ctx.fillStyle = 'rgba(255, 40, 40, 0.45)';
		for (let y = 0; y < ROWS; y++) {
			for (let x = 0; x < COLS; x++) {
				if (mask[y * COLS + x]) {
					ctx.fillRect(x * CELL, y * CELL, CELL, CELL);
				}
			}
		}
	}

	function onContextMenu(e: MouseEvent) {
		e.preventDefault();
	}

	let maskedCount = $derived(mask.reduce((a, v) => a + v, 0));
</script>

<svelte:window on:mouseup={onPointerUp} />

<div class="mask-page">
	<div class="toolbar">
		<h2>Mask Editor</h2>
		<span class="info">{maskedCount} / {COLS * ROWS} cells masked</span>
		<div class="spacer"></div>

		<label class="brush-label">
			Brush
			<input type="range" min="1" max="8" bind:value={brushSize} class="brush-range" />
			<span class="brush-val">{brushSize}</span>
		</label>

		<button class="btn" onclick={clearMask}>Clear</button>
		<button class="btn" onclick={fillMask}>Fill All</button>
		<button class="btn primary" onclick={saveMask} disabled={!dirty || saving}>
			{saving ? 'Saving...' : 'Save'}
		</button>
		{#if statusMsg}
			<span class="status" class:error={statusMsg.startsWith('Error')}>{statusMsg}</span>
		{/if}
	</div>

	<p class="hint">
		Click/drag to mask. Right-click/Ctrl+click to unmask.
	</p>

	<div class="canvas-wrap">
		<video bind:this={videoEl} autoplay muted playsinline class="bg-video"></video>
		<canvas
			bind:this={canvas}
			width={W}
			height={H}
			onmousedown={onPointerDown}
			onmousemove={onPointerMove}
			oncontextmenu={onContextMenu}
			ontouchstart={onTouchStart}
			ontouchmove={onTouchMove}
			ontouchend={onTouchEnd}
		></canvas>
	</div>
</div>

<style>
	.mask-page {
		padding: 1rem;
		max-width: 720px;
		margin: 0 auto;
	}

	.toolbar {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		flex-wrap: wrap;
		margin-bottom: 0.5rem;
	}

	.toolbar h2 {
		margin: 0;
		font-size: 1.1rem;
		font-weight: 600;
	}

	.info {
		font-size: 0.8rem;
		color: var(--text2);
	}

	.spacer { flex: 1; }

	.hint {
		font-size: 0.78rem;
		color: var(--text3);
		margin: 0 0 0.75rem;
	}

	/* Brush size control */
	.brush-label {
		display: flex;
		align-items: center;
		gap: 0.35rem;
		font-size: 0.8rem;
		color: var(--text2);
	}

	.brush-range {
		width: 60px;
		accent-color: #2563eb;
	}

	.brush-val {
		min-width: 1.2em;
		text-align: center;
		font-variant-numeric: tabular-nums;
		color: var(--text);
	}

	/* Canvas area with video background */
	.canvas-wrap {
		position: relative;
		border: 1px solid var(--border);
		border-radius: 6px;
		overflow: hidden;
		background: #000;
		aspect-ratio: 640 / 360;
	}

	.bg-video {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		object-fit: contain;
	}

	canvas {
		position: absolute;
		inset: 0;
		display: block;
		width: 100%;
		height: 100%;
		cursor: crosshair;
		touch-action: none;
	}

	.btn {
		padding: 0.35rem 0.75rem;
		font-size: 0.8rem;
		border: 1px solid var(--border);
		border-radius: 5px;
		background: var(--input);
		color: var(--text);
		cursor: pointer;
		transition: background 0.15s;
	}

	.btn:hover { background: var(--hover); }

	.btn.primary {
		background: #2563eb;
		border-color: #2563eb;
		color: #fff;
	}

	.btn.primary:hover { background: #1d4ed8; }
	.btn.primary:disabled { opacity: 0.4; cursor: default; }

	.status {
		font-size: 0.8rem;
		color: #22c55e;
	}

	.status.error {
		color: #ef4444;
	}
</style>
