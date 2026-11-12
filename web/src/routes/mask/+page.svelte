<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { Go2rtcPlayer } from '$lib/player';
	import { t } from '$lib/i18n';

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
	let brushSize = $state(3); // radius in cells (1 = single cell, 2 = 3x3, etc.)
	let tool = $state<'mask' | 'erase' | 'pan'>('mask');
	let zoom = $state(1);
	let hover: [number, number] | null = null;
	let last: [number, number] | null = null;
	let undoStack: Uint8Array[] = [];
	let canUndo = $state(false);
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
		pushUndo();
		mask = new Uint8Array(COLS * ROWS);
		dirty = true;
		draw();
	}

	function fillMask() {
		pushUndo();
		mask = new Uint8Array(COLS * ROWS).fill(1);
		dirty = true;
		draw();
	}

	function cellFromEvent(e: PointerEvent): [number, number] | null {
		const rect = canvas.getBoundingClientRect();
		const x = Math.floor(((e.clientX - rect.left) * W) / rect.width / CELL);
		const y = Math.floor(((e.clientY - rect.top) * H) / rect.height / CELL);
		if (x < 0 || x >= COLS || y < 0 || y >= ROWS) return null;
		return [x, y];
	}

	function stamp(cx: number, cy: number): boolean {
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
		return changed;
	}

	// Stamp along a line so fast strokes leave no gaps.
	function paintLine(x0: number, y0: number, x1: number, y1: number) {
		const n = Math.max(Math.abs(x1 - x0), Math.abs(y1 - y0), 1);
		let changed = false;
		for (let i = 0; i <= n; i++) {
			const x = Math.round(x0 + ((x1 - x0) * i) / n);
			const y = Math.round(y0 + ((y1 - y0) * i) / n);
			if (stamp(x, y)) changed = true;
		}
		if (changed) dirty = true;
	}

	function pushUndo() {
		undoStack.push(mask.slice());
		if (undoStack.length > 30) undoStack.shift();
		canUndo = true;
	}

	function undo() {
		const prev = undoStack.pop();
		if (!prev) return;
		mask = prev;
		canUndo = undoStack.length > 0;
		dirty = true;
		draw();
	}

	function onPointerDown(e: PointerEvent) {
		if (tool === 'pan') return; // native scroll
		if (e.pointerType === 'mouse' && e.button !== 0 && e.button !== 2) return;
		e.preventDefault();
		canvas.setPointerCapture(e.pointerId);
		const cell = cellFromEvent(e);
		if (!cell) return;
		const erase = tool === 'erase' || e.button === 2 || e.ctrlKey || (e.buttons & 32) !== 0;
		brushValue = erase ? 0 : 1;
		pushUndo();
		painting = true;
		last = cell;
		hover = cell;
		paintLine(cell[0], cell[1], cell[0], cell[1]);
		draw();
	}

	function onPointerMove(e: PointerEvent) {
		const cell = cellFromEvent(e);
		hover = e.pointerType === 'touch' ? null : cell;
		if (painting && cell) {
			const from = last ?? cell;
			paintLine(from[0], from[1], cell[0], cell[1]);
			last = cell;
		}
		draw();
	}

	function onPointerUp(e: PointerEvent) {
		if (painting && undoStack.length && !dirty) {
			undoStack.pop(); // stroke changed nothing
			canUndo = undoStack.length > 0;
		}
		painting = false;
		last = null;
		if (e.pointerType === 'touch') hover = null;
		draw();
	}

	function onPointerLeave() {
		hover = null;
		draw();
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
		// Brush cursor (mouse / pen hover)
		if (hover && tool !== 'pan') {
			const r = brushSize - 1;
			ctx.strokeStyle = tool === 'erase' ? '#4ade80' : '#fff';
			ctx.lineWidth = 1.5;
			ctx.strokeRect((hover[0] - r) * CELL, (hover[1] - r) * CELL, (2 * r + 1) * CELL, (2 * r + 1) * CELL);
		}
	}

	function onContextMenu(e: MouseEvent) {
		e.preventDefault();
	}

	let maskedCount = $derived(mask.reduce((a, v) => a + v, 0));
</script>

<div class="mask-page">
	<div class="toolbar">
		<h2>{t('Mask Editor')}</h2>
		<span class="info">{maskedCount} / {COLS * ROWS} {t('cells masked')}</span>
		<div class="spacer"></div>

		<label class="brush-label">
			{t('Brush')}
			<input type="range" min="1" max="8" bind:value={brushSize} class="brush-range" />
			<span class="brush-val">{brushSize}</span>
		</label>

		<button class="btn" class:active={tool === 'mask'} onclick={() => { tool = 'mask'; draw(); }}>{t('Mask')}</button>
		<button class="btn" class:active={tool === 'erase'} onclick={() => { tool = 'erase'; draw(); }}>{t('Erase')}</button>
		<button class="btn" class:active={tool === 'pan'} onclick={() => tool = 'pan'}>{t('Pan')}</button>
		<label class="brush-label">
			{t('Zoom')}
			<input type="range" min="1" max="4" step="0.5" bind:value={zoom} class="brush-range" />
			<span class="brush-val">{zoom}x</span>
		</label>
		<button class="btn" onclick={undo} disabled={!canUndo}>{t('Undo')}</button>
		<button class="btn" onclick={clearMask}>{t('Clear')}</button>
		<button class="btn" onclick={fillMask}>{t('Fill All')}</button>
		<button class="btn primary" onclick={saveMask} disabled={!dirty || saving}>
			{saving ? t('Saving...') : t('Save')}
		</button>
		{#if statusMsg}
			<span class="status" class:error={statusMsg.startsWith('Error')}>{statusMsg}</span>
		{/if}
	</div>

	<p class="hint">
		{t('Click/drag to mask. Right-click/Ctrl+click or Erase mode to unmask.')}
	</p>

	<div class="canvas-wrap" class:panning={tool === 'pan'}>
		<div class="stage" style:width="{zoom * 100}%">
		<video bind:this={videoEl} autoplay muted playsinline webkit-playsinline class="bg-video"></video>
		<canvas
			bind:this={canvas}
			width={W}
			height={H}
			onpointerdown={onPointerDown}
			onpointermove={onPointerMove}
			onpointerup={onPointerUp}
			onpointercancel={onPointerUp}
			onpointerleave={onPointerLeave}
			oncontextmenu={onContextMenu}
		></canvas>
		</div>
	</div>
</div>

<style>
	.mask-page {
		padding: 0.75rem;
		max-width: 90%;
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
		width: 100px;
		height: 28px;
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
		overflow: auto;
		background: #000;
		max-height: 75vh;
		overscroll-behavior: contain;
	}

	.stage {
		position: relative;
		aspect-ratio: 640 / 360;
	}

	.bg-video {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		object-fit: contain;
		z-index: 0;
		/* Force own compositing layer — works around an iPad Safari quirk where a
		   stacked transparent canvas can prevent the underlying <video> frame
		   from being drawn. */
		transform: translateZ(0);
	}

	canvas {
		position: absolute;
		inset: 0;
		display: block;
		width: 100%;
		height: 100%;
		cursor: crosshair;
		touch-action: none;
		user-select: none;
		-webkit-user-select: none;
		z-index: 1;
		background: transparent;
	}

	.btn {
		padding: 0.5rem 1rem;
		font-size: 0.85rem;
		border: 1px solid var(--border);
		border-radius: 5px;
		background: var(--input);
		color: var(--text);
		cursor: pointer;
		transition: background 0.15s;
	}

	.panning canvas { touch-action: pan-x pan-y; cursor: grab; }

	.btn:hover { background: var(--hover); }
	.btn.active { background: #2563eb; color: #fff; border-color: #2563eb; }

	.btn.primary {
		background: #2563eb;
		border-color: #2563eb;
		color: #fff;
	}

	.btn.primary:hover { background: #1d4ed8; }
	.btn.primary:disabled, .btn:disabled { opacity: 0.4; cursor: default; }

	@media (pointer: coarse) {
		.mask-page { max-width: 100%; padding: 0.5rem; }
		.btn { min-height: 44px; min-width: 44px; padding: 0.5rem 0.9rem; }
		.brush-range { width: 130px; height: 44px; }
		.toolbar { gap: 0.5rem; }
	}

	.status {
		font-size: 0.8rem;
		color: #22c55e;
	}

	.status.error {
		color: #ef4444;
	}
</style>
