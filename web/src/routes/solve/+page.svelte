<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from '$lib/i18n';

	const W = 640;
	const H = 360;
	/// Click within this many image-pixels of a star deletes it instead of adding.
	const HIT_RADIUS = 8;

	type SolveStar = { x: number; y: number; flux?: number; manual?: boolean };

	let canvas: HTMLCanvasElement;
	let ctx: CanvasRenderingContext2D;
	let frameCanvas: HTMLCanvasElement; // offscreen, holds the captured grayscale frame

	let stars = $state<SolveStar[]>([]);
	let threshold = $state<number | null>(null);
	let captured = $state(false);
	let capturing = $state(false);
	let solving = $state(false);
	let statusMsg = $state('');

	type Solution = { ra: number; dec: number; roll: number; votes: number };
	let solution = $state<Solution | null>(null);
	let noSolution = $state(false);

	let starCount = $derived(stars.length);

	onMount(() => {
		ctx = canvas.getContext('2d')!;
		frameCanvas = document.createElement('canvas');
		frameCanvas.width = W;
		frameCanvas.height = H;
		draw();
	});

	async function capture() {
		capturing = true;
		statusMsg = '';
		solution = null;
		noSolution = false;
		try {
			const res = await fetch('/api/solve/capture');
			if (!res.ok) {
				statusMsg = `Error: ${res.status}`;
				return;
			}
			const buf = new Uint8Array(await res.arrayBuffer());
			// [u32 LE meta length][meta JSON][raw WIDTH×HEIGHT luma]
			const metaLen = buf[0] | (buf[1] << 8) | (buf[2] << 16) | (buf[3] << 24);
			const metaStr = new TextDecoder().decode(buf.subarray(4, 4 + metaLen));
			const meta = JSON.parse(metaStr);
			const luma = buf.subarray(4 + metaLen);

			// Render grayscale luma into the offscreen frame canvas.
			const fctx = frameCanvas.getContext('2d')!;
			const img = fctx.createImageData(W, H);
			for (let i = 0; i < W * H; i++) {
				const v = luma[i];
				img.data[i * 4] = v;
				img.data[i * 4 + 1] = v;
				img.data[i * 4 + 2] = v;
				img.data[i * 4 + 3] = 255;
			}
			fctx.putImageData(img, 0, 0);

			threshold = meta.threshold ?? null;
			stars = (meta.stars as number[][]).map(([x, y, flux]) => ({ x, y, flux }));
			captured = true;
			draw();
		} catch (e) {
			statusMsg = `Error: ${e}`;
		} finally {
			capturing = false;
		}
	}

	function pointFromEvent(e: MouseEvent): [number, number] {
		const rect = canvas.getBoundingClientRect();
		const x = ((e.clientX - rect.left) / rect.width) * W;
		const y = ((e.clientY - rect.top) / rect.height) * H;
		return [x, y];
	}

	function onClick(e: MouseEvent) {
		if (!captured) return;
		const [x, y] = pointFromEvent(e);

		// Hit-test existing stars; click near one removes it.
		let nearest = -1;
		let nearestD2 = HIT_RADIUS * HIT_RADIUS;
		for (let i = 0; i < stars.length; i++) {
			const dx = stars[i].x - x;
			const dy = stars[i].y - y;
			const d2 = dx * dx + dy * dy;
			if (d2 <= nearestD2) {
				nearestD2 = d2;
				nearest = i;
			}
		}

		if (nearest >= 0) {
			stars = stars.filter((_, i) => i !== nearest);
		} else {
			stars = [...stars, { x, y, manual: true }];
		}
		// Editing invalidates the previous solution.
		solution = null;
		noSolution = false;
		draw();
	}

	function clearStars() {
		stars = [];
		solution = null;
		noSolution = false;
		draw();
	}

	async function solve() {
		if (stars.length < 4) {
			statusMsg = t('Need at least 4 stars');
			return;
		}
		solving = true;
		statusMsg = '';
		solution = null;
		noSolution = false;
		try {
			const res = await fetch('/api/solve', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({ stars: stars.map((s) => [s.x, s.y]) }),
			});
			const data = await res.json();
			if (!data.ok) {
				statusMsg = `Error: ${data.error ?? 'solve failed'}`;
			} else if (data.solution === null) {
				noSolution = true;
			} else {
				solution = { ra: data.ra, dec: data.dec, roll: data.roll, votes: data.votes };
			}
		} catch (e) {
			statusMsg = `Error: ${e}`;
		} finally {
			solving = false;
		}
	}

	function draw() {
		if (!ctx) return;
		ctx.clearRect(0, 0, W, H);
		if (captured && frameCanvas) {
			ctx.drawImage(frameCanvas, 0, 0);
		} else {
			ctx.fillStyle = '#000';
			ctx.fillRect(0, 0, W, H);
		}

		for (const s of stars) {
			if (s.manual) {
				// Manually added — green cross
				ctx.strokeStyle = '#2ecc71';
				ctx.lineWidth = 1;
				ctx.beginPath();
				ctx.moveTo(s.x - 4, s.y);
				ctx.lineTo(s.x + 4, s.y);
				ctx.moveTo(s.x, s.y - 4);
				ctx.lineTo(s.x, s.y + 4);
				ctx.stroke();
			} else {
				// Auto-extracted — yellow circle
				ctx.strokeStyle = '#ff0';
				ctx.lineWidth = 1;
				ctx.beginPath();
				ctx.arc(s.x, s.y, 4, 0, Math.PI * 2);
				ctx.stroke();
			}
		}
	}
</script>

<div class="solve-page">
	<div class="toolbar">
		<h2>{t('Star Editor')}</h2>
		<span class="info">
			{starCount} {t('stars')}{threshold != null ? ` · thr=${threshold}` : ''}
		</span>
		<div class="spacer"></div>

		<button class="btn" onclick={capture} disabled={capturing}>
			{capturing ? t('Capturing…') : t('Capture')}
		</button>
		<button class="btn" onclick={clearStars} disabled={!captured}>{t('Clear')}</button>
		<button class="btn primary" onclick={solve} disabled={!captured || solving || stars.length < 4}>
			{solving ? t('Solving…') : t('Solve')}
		</button>
		{#if statusMsg}
			<span class="status error">{statusMsg}</span>
		{/if}
	</div>

	<p class="hint">
		{t('Capture a frame, then click empty space to add a star or click a marker to remove it. Need ≥ 4 stars to solve.')}
	</p>

	<div class="canvas-wrap">
		<canvas bind:this={canvas} width={W} height={H} onclick={onClick}></canvas>
		{#if !captured}
			<div class="placeholder">{t('Press Capture to grab a frame')}</div>
		{/if}
	</div>

	{#if solution}
		<div class="result">
			<div class="result-row"><span class="result-label">RA</span><span class="result-val">{solution.ra.toFixed(3)}°</span></div>
			<div class="result-row"><span class="result-label">Dec</span><span class="result-val">{solution.dec.toFixed(3)}°</span></div>
			<div class="result-row"><span class="result-label">Roll</span><span class="result-val">{solution.roll.toFixed(2)}°</span></div>
			<div class="result-row"><span class="result-label">Votes</span><span class="result-val">{solution.votes}</span></div>
		</div>
	{:else if noSolution}
		<div class="result no-solution">{t('No solution found')}</div>
	{/if}
</div>

<style>
	.solve-page {
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
		font-family: monospace;
	}

	.spacer { flex: 1; }

	.hint {
		font-size: 0.78rem;
		color: var(--text3);
		margin: 0 0 0.75rem;
	}

	.canvas-wrap {
		position: relative;
		border: 1px solid var(--border);
		border-radius: 6px;
		overflow: hidden;
		background: #000;
		aspect-ratio: 640 / 360;
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

	.placeholder {
		position: absolute;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		color: var(--text3);
		font-size: 0.85rem;
		pointer-events: none;
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

	.btn:hover { background: var(--hover); }
	.btn:disabled { opacity: 0.4; cursor: default; }

	.btn.primary {
		background: #2563eb;
		border-color: #2563eb;
		color: #fff;
	}
	.btn.primary:hover { background: #1d4ed8; }
	.btn.primary:disabled { opacity: 0.4; cursor: default; }

	.status { font-size: 0.8rem; color: #22c55e; }
	.status.error { color: #ef4444; }

	.result {
		margin-top: 0.75rem;
		display: flex;
		gap: 1.25rem;
		flex-wrap: wrap;
		background: var(--card);
		border-radius: 8px;
		padding: 0.6rem 0.9rem;
		font-family: monospace;
	}

	.result-row {
		display: flex;
		flex-direction: column;
		gap: 0.1rem;
	}

	.result-label {
		font-size: 0.65rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text3);
	}

	.result-val {
		font-size: 1rem;
		color: var(--text);
	}

	.no-solution {
		color: #f90;
		font-family: inherit;
	}
</style>
