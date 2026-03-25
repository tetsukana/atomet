<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { Go2rtcPlayer } from '$lib/player';
	import type { PlayerMode, PlayerStatus } from '$lib/player';
	import { AtometWebSocket } from '$lib/ws';
	import type { AppState, SysStat, DetectionDebug, MeteorEvent } from '$lib/types';
	import { draw } from 'svelte/transition';
	import { zoom as d3Zoom, zoomIdentity } from 'd3-zoom';
	import { select } from 'd3-selection';

	// Stream name from URL query param, default 'main' (matches go2rtc.yaml)
	const streamName = typeof window !== 'undefined'
		? (new URLSearchParams(location.search).get('stream') ?? 'main')
		: 'main';

	let videoEl = $state<HTMLVideoElement | undefined>(undefined);
	let mode = $state<PlayerMode | null>(null);
	let playerStatus = $state<PlayerStatus>('idle');
	let wsConnected = $state(false);
	let appState = $state<AppState | null>(null);

	const MAX_HISTORY = 120;
	type StatPoint = { cpu: number; mem_pct: number };
	let statHistory = $state<StatPoint[]>([]);
	let statLast = $derived(statHistory[statHistory.length - 1] ?? null);

	let player = $state<Go2rtcPlayer | null>(null);
	let ws: AtometWebSocket;
	let showDebug = $state(false);
	let detectionDebug = $state<DetectionDebug | null>(null);
	let ispExprUs = $state(0);
	let ispAgain = $state(0);
	let ispDgain = $state(0);
	let ispsDgain = $state(0);
	let ispItMax = $state(0);
	let ispAgMax = $state(0);
	let ispiDgMax = $state(0);
	let ispsDgMax = $state(0);
	let ispItMin = $state(0);
	let ispAgMin = $state(0);
	let ispiDgMin = $state(0);
	let ispsDgMin = $state(0);
	let ispiDgi = $state(0);
	let ispAgi = $state(0);
	let activeTab = $state("settings");
	let stageEl = $state<HTMLDivElement | undefined>(undefined);
	let zoomTransform = $state('');
	let bandwidth = $state(0);
	let histZoomK = 1;
	let histZoomX = 0;
	let histLog = $state(true);

	let histogram = new Array(256).fill(0);
    let canvas;
    let ctx;
    let observer;

    let width = 0;
    let height = 0;

	const MAX_METEOR_LOG = 20;
	let meteorLog = $state<MeteorEvent[]>([]);

	$effect(() => {
		if (!videoEl || !player) return;
		player.mount(videoEl);
	});

	onMount(() => {
		player = new Go2rtcPlayer(streamName);
		player.onModeChange = (m) => { mode = m; };
		player.onStatusChange = (s) => { playerStatus = s; };
		player.onBandwidth = (bps) => { bandwidth = bps; };

		ws = new AtometWebSocket();
		ws.onStatusChange = (c) => { wsConnected = c; };
		ws.onAppState = (s) => { appState = s; };
		ws.onSysStat = (s) => {
			const point: StatPoint = {
				cpu: s.cpu,
				mem_pct: s.mem_total > 0 ? (s.mem_used / s.mem_total) * 100 : 0,
			};
			statHistory = [...statHistory.slice(-(MAX_HISTORY - 1)), point];
		};
		ws.onIspStat = (s) => {
			ispExprUs = s.it;
			ispAgain = s.ag;
			ispDgain = s.idg;
			ispsDgain = s.sdg;
			ispItMax = s.max_it;
			ispAgMax = s.max_ag;
			ispiDgMax = s.max_idg;
			ispsDgMax = s.max_sdg;
			ispItMin = s.min_it;
			ispAgMin = s.min_ag;
			ispiDgMin = s.min_idg;
			ispsDgMin = s.min_sdg;
			histogram = s.histogram;
			ispiDgi = s.idg_i;
			ispAgi = s.ag_i;
			draw_hist(histogram);
		}
		ws.onDetectionDebug = (d) => { detectionDebug = d; };
		ws.onMeteorEvent = (e) => {
			meteorLog = [e, ...meteorLog.slice(0, MAX_METEOR_LOG - 1)];
		};
		ws.connect();

		// d3-zoom on stage
		if (stageEl) {
			const el = stageEl;

			const zoomBehavior = d3Zoom()
				.scaleExtent([1, 20])
				.on('zoom', (event: any) => {
					const { x, y, k } = event.transform;

					// Constrain pan so content always covers the viewport
					const w = el.clientWidth;
					const h = el.clientHeight;
					const cx = Math.min(0, Math.max(x, w - w * k));
					const cy = Math.min(0, Math.max(y, h - h * k));
					zoomTransform = `translate(${cx}px,${cy}px) scale(${k})`;
				});

			const sel = select(stageEl);
			sel.call(zoomBehavior as any);

			// Double-click to reset
			sel.on('dblclick.zoom', () => {
				sel.transition().duration(300).call(zoomBehavior.transform as any, zoomIdentity);
			});
		}

		ctx = canvas.getContext("2d");

        observer = new ResizeObserver(entries => {
            const rect = entries[0].contentRect;

            width = rect.width;
            height = rect.height;

            canvas.width = width;
            canvas.height = height;

            draw_hist(histogram);
        });

        observer.observe(canvas);

		// d3-zoom on histogram (X-axis only)
		const histZoom = d3Zoom()
			.scaleExtent([1, 32])
			.on('zoom', (event: any) => {
				const k = event.transform.k;
				const cw = canvas.clientWidth;
				// Clamp X so content covers the viewport
				const x = Math.min(0, Math.max(event.transform.x, cw - cw * k));
				histZoomK = k;
				histZoomX = x;
				draw_hist(histogram);
			});
		select(canvas).call(histZoom as any);
		select(canvas).on('dblclick.zoom', () => {
			select(canvas).transition().duration(300).call(histZoom.transform as any, zoomIdentity);
		});
	});

	onDestroy(() => {
		player?.unmount();
		ws?.disconnect();
		observer.disconnect();
	});

	function send(type: string, value: unknown) {
		ws?.send({ type, value });
	}

	const modeBadge: Record<NonNullable<PlayerMode>, { label: string; color: string }> = {
		webrtc: { label: 'WebRTC', color: '#2ecc71' },
		mse:    { label: 'MSE',    color: '#3498db' },
		hls:    { label: 'HLS',    color: '#e67e22' },
		mjpeg:  { label: 'MJPEG', color: '#95a5a6' },
	};

	function cssVar(name) {
    	return getComputedStyle(document.documentElement)
        	.getPropertyValue(name)
        	.trim();
	}

	function draw_hist(hist: number[]) {
		if (!width || !height) return;

		const log = histLog;
		const pad = width * 0.05;
		const plotW = width - pad * 2;
		const n = hist.length;
		const k = histZoomK;
		const tx = histZoomX;

		const binW = (plotW * k) / n;
		const x0 = pad + tx;

		// Visible bin range
		const iMin = Math.max(0, Math.floor(-tx / binW));
		const iMax = Math.min(n - 1, Math.ceil((width - pad - tx) / binW));

		// Y-scale from visible range
		let rawMax = 0;
		for (let i = iMin; i <= iMax; i++) rawMax = Math.max(rawMax, hist[i]);
		if (rawMax === 0) rawMax = 1;
		const yMax = log ? Math.log1p(rawMax) : rawMax;

		const yVal = (v: number) => log ? Math.log1p(v) : v;
		const toY = (v: number) => height - (yVal(v) / yMax) * height;

		ctx.clearRect(0, 0, width, height);

		// Grid lines at 0, 64, 128, 192, 255
		ctx.strokeStyle = cssVar("--text3");
		ctx.beginPath();
		for (const v of [0, 64, 128, 192, 255]) {
			const x = x0 + v * binW;
			if (x >= pad && x <= width - pad) {
				ctx.moveTo(x, 0);
				ctx.lineTo(x, height);
			}
		}
		ctx.stroke();

		// Step line
		ctx.strokeStyle = cssVar("--text");
		ctx.beginPath();
		let started = false;
		for (let i = 0; i < n; i++) {
			const lx = x0 + i * binW;
			const rx = lx + binW;
			if (rx < pad || lx > width - pad) continue;
			const y = toY(hist[i]);
			if (!started) { ctx.moveTo(lx, y); started = true; }
			else { ctx.lineTo(lx, toY(hist[i - 1])); ctx.lineTo(lx, y); }
			if (i === n - 1) ctx.lineTo(rx, y);
		}
		ctx.stroke();

		// Bin labels at zoom > 4x
		if (k >= 4) {
			ctx.fillStyle = cssVar("--text3");
			ctx.font = '9px monospace';
			ctx.textAlign = 'center';
			const step = k >= 16 ? 1 : k >= 8 ? 2 : 4;
			for (let i = iMin; i <= iMax; i += step) {
				const cx = x0 + (i + 0.5) * binW;
				if (cx >= pad && cx <= width - pad) {
					ctx.fillText(String(i), cx, height - 2);
				}
			}
		}
	}

	function fixedToFloat(value: number, fracBits: number): number {
    	return value / (1 << fracBits);
	}
</script>

<div class="page">

	<!-- ── Video stage ───────────────────────────── -->
	<div class="stage" bind:this={stageEl}>
	<div class="zoom-content" style="transform:{zoomTransform}">
		<video bind:this={videoEl} autoplay muted playsinline></video>
	</div><!-- zoom-content (video only) -->

		{#if playerStatus === 'connecting'}
			<div class="overlay">
				<span class="spinner"></span>
				<span>Connecting…</span>
			</div>
		{:else if playerStatus === 'error'}
			<div class="overlay">
				<span class="err-icon">⚠</span>
				<span>Reconnecting…</span>
			</div>
		{/if}

		<div class="hud-top">
			<span class="stream-label">{streamName}</span>
			{#if mode}
				<span class="badge" style="background:{modeBadge[mode].color}">
					{modeBadge[mode].label}
				</span>
			{/if}
			{#if bandwidth > 0}
				<span class="badge bw">{bandwidth >= 1048576 ? (bandwidth / 1048576).toFixed(1) + ' MB/s' : (bandwidth / 1024).toFixed(0) + ' KB/s'}</span>
			{/if}
		</div>

		<div class="ws-dot" class:on={wsConnected}></div>

		{#if showDebug}
			<svg class="det-overlay" viewBox="0 0 640 360" preserveAspectRatio="none">
				{#each meteorLog.slice(0, 5) as meteor}
					{#each meteor.trajectory.slice(0, -1) as pt, i}
						<line
							x1={pt[0]} y1={pt[1]}
							x2={meteor.trajectory[i + 1][0]} y2={meteor.trajectory[i + 1][1]}
							stroke="#0f0" stroke-width="1.5" opacity="0.6"
						/>
					{/each}
					{#if meteor.trajectory.length > 0}
						{@const last = meteor.trajectory[meteor.trajectory.length - 1]}
						<text x={last[0] + 4} y={last[1] - 4} fill="#0f0" font-size="9" font-family="monospace">
							#{meteor.id} {meteor.speed.toFixed(1)}px/f
						</text>
					{/if}
				{/each}
			</svg>
		{/if}

		{#if showDebug}
			<div class="isp-info">
				<span>IT {ispExprUs}</span>
				<span>AG {ispAgain}</span>
				<span>AGi {ispAgi}</span>
				<span>DG {ispDgain}</span>
				<span>DGi {ispiDgi}</span>
				<span>sDG {ispsDgain}</span>
				<span>ITMAX {ispItMax}</span>
				<span>AGMAX {ispAgMax}</span>
				<span>iDGMAX {ispiDgMax}</span>
				<span>sDGMAX {ispsDgMax}</span>
				<span>ITMIN {ispItMin}</span>
				<span>AGMIN {ispAgMin}</span>
				<span>iDGMIN {ispiDgMin}</span>
				<span>sDGMIN {ispsDgMin}</span>
			</div>
		{/if}

		{#if showDebug && detectionDebug}
			<svg class="det-overlay" viewBox="0 0 640 360" preserveAspectRatio="none">
				{#each detectionDebug.b as [bx, by, bw, bh]}
					<rect
						x={bx * 8} y={by * 8}
						width={bw * 8} height={bh * 8}
						fill="none" stroke="#ff0" stroke-width="1.5" stroke-dasharray="4 2" opacity="0.8"
					/>
				{/each}
				{#each detectionDebug.l as [x0, y0, x1, y1]}
					<line {x0} {y0} {x1} {y1} stroke="#0ff" stroke-width="1.5" opacity="0.85" />
				{/each}
				{#each detectionDebug.t as [id, mx, my, cnt]}
					<circle cx={mx} cy={my} r={cnt >= 2 ? 5 : 3}
						fill={cnt >= 2 ? '#f80' : '#f84'} opacity="0.9" />
					<text x={mx + 6} y={my - 3} fill="#f80" font-size="9" font-family="monospace">
						#{id}
					</text>
				{/each}
				{#if detectionDebug.c.length > 0}
					{@const confirmedIds = detectionDebug.c}
					{#each detectionDebug.t.filter(([id]) => confirmedIds.includes(id)) as [, mx, my]}
						<circle cx={mx} cy={my} r="8" fill="#0f0" opacity="0.7" />
					{/each}
				{/if}
				<text x="4" y="14" fill="#0f0" font-size="11" font-family="monospace">
					zs={detectionDebug.mzs?.toFixed(1) ?? '?'} zt={detectionDebug.mzt?.toFixed(1) ?? '?'}
				</text>
			</svg>
		{/if}
	</div><!-- stage -->

	<!-- ── Sidebar ────────────────────────────────── -->
	<aside class="sidebar">
	<div class="tab-bar">
    <button class="tab" class:active={activeTab === 'settings'} onclick={() => activeTab = 'settings'}>Settings</button>
    <button class="tab" class:active={activeTab === 'log'} onclick={() => activeTab = 'log'}>Log {meteorLog.length > 0 ? `(${meteorLog.length})` : ''}</button>
  	</div>

  <!-- スクロール領域 -->
  <div class="sidebar-scroll">
	<div style={`display:${activeTab === 'settings' ? "flex" : "none"}`} class="settings-bar">
		<!-- Mini stats chart -->
		{#if statHistory.length > 1}
			{@const W = MAX_HISTORY}
			{@const H = 44}
			{@const pts = (key: 'cpu' | 'mem_pct') => {
				const n = statHistory.length;
				return statHistory
					.map((p, i) => `${(i / (n - 1)) * W},${H - (p[key] / 100) * H}`)
					.join(' ');
			}}
			<div class="stats-block">
				<div class="stats-labels">
					<span style="color:#2ecc71">CPU</span>
					{#if statLast}<span class="stats-val">{statLast.cpu.toFixed(1)}%</span>{/if}
					<span style="color:#3498db; margin-left:auto">MEM</span>
					{#if statLast}<span class="stats-val">{statLast.mem_pct.toFixed(1)}%</span>{/if}
				</div>
				<svg viewBox="0 0 {W} {H}" preserveAspectRatio="none" class="stats-svg">
					{#each [25, 50, 75] as pct}
						<line x1="0" y1={H - (pct / 100) * H} x2={W} y2={H - (pct / 100) * H}
							stroke="var(--chart-grid)" stroke-width="0.5" />
					{/each}
					<polyline points={pts('cpu')}     fill="none" stroke="#2ecc71" stroke-width="1.5" stroke-linejoin="round" />
					<polyline points={pts('mem_pct')} fill="none" stroke="#3498db" stroke-width="1.5" stroke-linejoin="round" />
				</svg>
			</div>
		{/if}

		<div class="stats-block">
			<div class="stats-labels">
				<span class="ctrl-heading">HISTOGRAM</span>
				<button class="hist-log-btn" class:active={histLog}
					onclick={() => { histLog = !histLog; draw_hist(histogram); }}>LOG</button>
			</div>
			<canvas bind:this={canvas} class="hist"></canvas>
		</div>

		<!-- Controls -->
		{#if appState}
			<div class="ctrl-group">
				<span class="ctrl-heading">View</span>
				<label class="ctrl-row">
					<span>Debug overlay</span>
					<input type="checkbox" bind:checked={showDebug} />
				</label>
			</div>

			<div class="ctrl-group">
				<span class="ctrl-heading">Camera</span>
				<label class="ctrl-row">
					<span>Auto Day / Night</span>
					<input type="checkbox" checked={appState.auto_daynight}
						onchange={(e) => send('set_auto_daynight', e.currentTarget.checked)} />
				</label>
				<label class="ctrl-row">
					<span>Night Mode</span>
					<input type="checkbox" checked={appState.night_mode}
						onchange={(e) => send('set_night_mode', e.currentTarget.checked)} />
				</label>
				<label class="ctrl-row">
					<span>LED</span>
					<input type="checkbox" checked={appState.led_on}
						onchange={(e) => send('set_led', e.currentTarget.checked)} />
				</label>
				<label class="ctrl-row">
					<span>IR LED</span>
					<input type="checkbox" checked={appState.irled_on}
						onchange={(e) => send('set_irled', e.currentTarget.checked)} />
				</label>
				<label class="ctrl-row">
					<span>IR Cut</span>
					<input type="checkbox" checked={appState.ircut_on}
						onchange={(e) => send('set_ircut', e.currentTarget.checked)} />
				</label>

				<div class="ctrl-row">
					<span>FPS</span>
					<input type="range" min="5" max="25" step="5"
						value={appState.fps}
						oninput={(e) => {send('set_fps', +e.currentTarget.value); appState.fps = +e.currentTarget.value;}} />
					<span class="slider-val">{appState.fps}<span class="slider-unit">fps</span></span>
				</div>

				<label class="ctrl-row">
					<span>Auto Exposure</span>
					<input type="checkbox" bind:checked={appState.ae_enable}
						onchange={(e) => send('set_ae_enable', e.currentTarget.checked)} />
				</label>

				<div class="slider-row">
					<span class="slider-label">IT</span>
					<input type="range" min={ispItMin} max={ispItMax} step="1" disabled={appState.ae_enable}
						value={ispExprUs}
						oninput={(e) => send('set_exposure_us', +e.currentTarget.value)} />
					<span class="slider-val">{ispExprUs}<span class="slider-unit">ms</span></span>
				</div>
				<div class="slider-row">
					<span class="slider-label">AG</span>
					<input type="range" min={ispAgMin} max={ispAgMax} step="1" disabled={appState.ae_enable}
						value={ispAgain} 
						oninput={(e) => send('set_analog_gain', +e.currentTarget.value)} />
					<span class="slider-val">{ispAgain}<span class="slider-unit"></span></span>
				</div>
				<div class="slider-row">
					<span class="slider-label">DG</span>
					<input type="range" min={ispiDgMin} max={ispiDgMax} step="1" disabled={appState.ae_enable}
						value={ispDgain}
						oninput={(e) => send('set_digital_gain', +e.currentTarget.value)} />
					<span class="slider-val">{ispDgain}<span class="slider-unit"></span></span>
				</div>
			</div>

			<div class="ctrl-group">
				<span class="ctrl-heading">Functions</span>
				<label class="ctrl-row">
					<span>Recording</span>
					<input type="checkbox" checked={appState.record_enabled}
						onchange={(e) => send('set_record', e.currentTarget.checked)} />
				</label>
				<label class="ctrl-row">
					<span>Detection</span>
					<input type="checkbox" checked={appState.detection_enabled}
						onchange={(e) => send('set_detection', e.currentTarget.checked)} />
				</label>
			</div>

			<div class="ctrl-group">
				<span class="ctrl-heading">OSD</span>
				<label class="ctrl-row">
					<span>Timestamp</span>
					<input type="checkbox" checked={appState.show_timestamp}
						onchange={(e) => send('set_show_timestamp', e.currentTarget.checked)} />
				</label>
			</div>
		{/if}

	</div>
			<!-- ── Detection log (full width) ───────────────── -->
	<div style={`display:${activeTab === 'log' ? "flex" : "none"}`}  class="settings-bar">
	{#if meteorLog.length > 0}
		<div class="meteor-log">
			<div class="log-header">
				<span class="log-title">Meteors</span>
				<span class="log-count">{meteorLog.length}</span>
				<button class="log-clear" onclick={() => { meteorLog = []; }}>clear</button>
			</div>
			{#each meteorLog as m}
				<div class="log-entry">
					<span class="log-ts">{m.ts.slice(9, 11)}:{m.ts.slice(11, 13)}:{m.ts.slice(13, 15)}</span>
					<span class="log-id">#{m.id}</span>
					<span class="log-speed">{m.speed.toFixed(1)} px/f</span>
					<span class="log-dur">{m.frames} fr</span>
					<a class="log-img" href="/files/detections/{m.ts}.png" target="_blank" rel="noopener noreferrer" title="View stack image">
						<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><polyline points="21 15 16 10 5 21"/>
						</svg>
					</a>
				</div>
			{/each}
		</div>
	{/if}
		</div>
	  </div>
	</aside>

</div>

<style>
	/* ── Grid layout ──────────────────────────── */
	.page {
		max-width: 90%;
		margin: 0 auto;
		padding: 0.75rem;
		display: grid;
		grid-template-columns: 1fr 248px;
		grid-template-rows: auto auto;
		grid-template-areas:
			"stage   sidebar"
			"log     sidebar";
		gap: 0.75rem;
		align-items: start;
	}

	.stage       { grid-area: stage; }
	.sidebar     { grid-area: sidebar; }
	.meteor-log  { grid-area: log; }

	@media (max-width: 680px) {
		.page {
			grid-template-columns: 1fr;
			grid-template-areas:
				"stage"
				"sidebar"
				"log";
		}
	}

	/* ── Stage ──────────────────────────────── */
	.stage {
		position: relative;
		background: #000;
		border-radius: 8px;
		overflow: hidden;
		aspect-ratio: 16 / 9;
		touch-action: none; /* d3-zoom handles touch */
	}

	.zoom-content {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		transform-origin: 0 0;
		will-change: transform;
	}

	video {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		object-fit: contain;
	}

	.det-overlay {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		pointer-events: none;
	}

	.overlay {
		position: absolute;
		inset: 0;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 0.5rem;
		background: rgba(0, 0, 0, 0.6);
		font-size: 0.9rem;
		color: #ccc;
	}

	.spinner {
		display: block;
		width: 28px;
		height: 28px;
		border: 3px solid rgba(255, 255, 255, 0.12);
		border-top-color: #fff;
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
	}
	@keyframes spin { to { transform: rotate(360deg); } }

	.err-icon { font-size: 1.6rem; }

	.hud-top {
		position: absolute;
		top: 0.5rem;
		left: 0.5rem;
		display: flex;
		align-items: center;
		gap: 0.4rem;
	}

	.stream-label {
		font-size: 0.7rem;
		background: rgba(0, 0, 0, 0.55);
		padding: 0.15rem 0.4rem;
		border-radius: 4px;
		color: #ccc;
	}

	.badge {
		font-size: 0.7rem;
		font-weight: 600;
		padding: 0.15rem 0.4rem;
		border-radius: 4px;
		color: #fff;
	}
	.badge.bw {
		background: rgba(0, 0, 0, 0.55);
		color: #ccc;
		font-weight: 400;
		font-family: monospace;
	}

	.isp-info {
		position: absolute;
		bottom: 0.5rem;
		left: 0.5rem;
		display: flex;
		gap: 0.6rem;
		font-size: 0.7rem;
		font-family: monospace;
		color: #0f0;
		background: rgba(0, 0, 0, 0.6);
		padding: 0.15rem 0.5rem;
		border-radius: 4px;
		pointer-events: none;
	}

	.ws-dot {
		position: absolute;
		top: 0.55rem;
		right: 0.55rem;
		width: 9px;
		height: 9px;
		border-radius: 50%;
		background: #c0392b;
		transition: background 0.3s;
	}
	.ws-dot.on { background: #27ae60; }

	/* ── Sidebar ────────────────────────────── */
	.sidebar {
		position: sticky;
		top: 0.75rem;
		max-height: calc(100vh - 1.5rem - 48px);
		display: flex;
		flex-direction: column;
  		overflow: hidden; /* sidebar自体はスクロールさせない */
	}

	.tab-bar {
	  	display: flex;
	  	flex-shrink: 0;
	  	border-bottom: 1px solid var(--border);
	}

	.tab {
	  	flex: 1;
	  	padding: 6px;
	  	background: none;
	  	border: none;
		border-bottom: 2px solid transparent;
		margin-bottom: -1px;
		color: var(--text3);
	  	cursor: pointer;
	  	font-size: 0.75rem;
	}

	.tab:hover { color: var(--text2); }

	.tab.active {
	  	color: var(--text);
		border-bottom-color: var(--text);
	}

	.sidebar-scroll {
	  	flex: 1;
	  	overflow-y: auto;
	  	min-height: 0;
	}

	.settings-bar {
    	flex-direction: column;
		gap: 0.5rem;
	}

	/* Mini stats */
	.stats-block {
		background: var(--card);
		border-radius: 8px;
		padding: 0.5rem 0.6rem;
		display: flex;
		flex-direction: column;
		gap: 0.3rem;
	}

	.stats-labels {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		font-size: 0.7rem;
	}

	.stats-val { color: var(--text3); }

	.stats-svg {
		display: block;
		width: 100%;
		height: 44px;
	}

	.hist {
		display: block;
		width: 100%;
		height: 100px;
		border-radius: 4px;
	}

	.hist-log-btn {
		margin-left: auto;
		font-size: 0.6rem;
		font-family: monospace;
		font-weight: 600;
		padding: 0.05rem 0.35rem;
		border: 1px solid var(--border);
		border-radius: 3px;
		background: none;
		color: var(--text3);
		cursor: pointer;
	}
	.hist-log-btn.active {
		background: #1a5e34;
		border-color: #2ecc71;
		color: #2ecc71;
	}

	/* Control groups */
	.ctrl-group {
		background: var(--card);
		border-radius: 8px;
		padding: 0.4rem 0.6rem 0.5rem;
		display: flex;
		flex-direction: column;
	}

	.ctrl-heading {
		font-size: 0.65rem;
		font-weight: 600;
		letter-spacing: 0.07em;
		text-transform: uppercase;
		color: var(--text3);
		padding-bottom: 0.35rem;
		margin-bottom: 0.1rem;
	}

	.ctrl-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 0.38rem 0;
		font-size: 0.85rem;
		color: var(--text);
		cursor: pointer;
		border-top: 1px solid var(--border2);
	}

	.ctrl-row:first-of-type { border-top: none; }

	/* Custom toggle */
	.ctrl-row input[type="checkbox"] {
		width: 36px;
		height: 20px;
		appearance: none;
		background: var(--toggle-off);
		border-radius: 10px;
		cursor: pointer;
		position: relative;
		flex-shrink: 0;
		transition: background 0.2s;
	}

	.ctrl-row input[type="checkbox"]::after {
		content: '';
		position: absolute;
		width: 14px;
		height: 14px;
		border-radius: 50%;
		background: var(--text3);
		top: 3px;
		left: 3px;
		transition: transform 0.2s, background 0.2s;
	}

	.ctrl-row input[type="checkbox"]:checked { background: #1a5e34; }
	.ctrl-row input[type="checkbox"]:checked::after {
		transform: translateX(16px);
		background: #2ecc71;
	}

	/* ── Exposure sliders ─────────────────────── */
	.slider-row {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		padding: 0.3rem 0;
		border-top: 1px solid var(--border2);
	}
	.slider-row:first-of-type { border-top: none; }

	.slider-label {
		font-size: 0.7rem;
		font-weight: 600;
		font-family: monospace;
		color: var(--text3);
		width: 1.6rem;
		flex-shrink: 0;
	}

	.slider-row input[type="range"] {
		flex: 1;
		height: 4px;
		appearance: none;
		background: var(--input);
		border-radius: 2px;
		cursor: pointer;
	}

	.slider-row input[type="range"]::-webkit-slider-thumb {
		appearance: none;
		width: 14px;
		height: 14px;
		border-radius: 50%;
		background: #2ecc71;
		cursor: pointer;
	}

	.slider-val {
		font-size: 0.7rem;
		font-family: monospace;
		color: var(--text2);
		text-align: right;
		flex-shrink: 0;
	}

	.slider-unit {
		color: var(--text4);
		font-size: 0.6rem;
	}

	/* ── Meteor log ─────────────────────────── */
	.meteor-log {
		background: var(--card);
		border-radius: 8px;
		padding: 0.5rem 0.6rem;
		font-size: 0.75rem;
		font-family: monospace;
	}

	.log-header {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		margin-bottom: 0.35rem;
		color: var(--text2);
	}

	.log-title  { font-weight: 600; color: #0f0; }
	.log-count  { background: var(--input); border-radius: 10px; padding: 0 0.4rem; color: var(--text3); }

	.log-clear {
		margin-left: auto;
		background: none;
		border: 1px solid var(--border);
		border-radius: 4px;
		color: var(--text3);
		cursor: pointer;
		font-size: 0.7rem;
		padding: 0.1rem 0.4rem;
	}
	.log-clear:hover { color: var(--text2); border-color: var(--text3); }

	.log-entry {
		display: flex;
		gap: 0.6rem;
		padding: 0.18rem 0;
		border-top: 1px solid var(--border2);
		color: var(--text);
		white-space: nowrap;
		overflow: hidden;
	}

	.log-ts    { color: var(--text4); flex-shrink: 0; }
	.log-id    { color: #0f0;         flex-shrink: 0; }
	.log-speed { color: #f80;         flex-shrink: 0; }
	.log-dur   { color: var(--text3); flex-shrink: 0; }
	.log-img   { color: var(--text3); flex-shrink: 0; display: flex; align-items: center; margin-left: auto; }
	.log-img:hover { color: var(--text); }
</style>
