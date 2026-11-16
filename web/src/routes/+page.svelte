<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { Go2rtcPlayer } from '$lib/player';
	import type { PlayerMode, PlayerStatus } from '$lib/player';
	import { AtometWebSocket } from '$lib/ws';
	import type { AppState, SysStat, DetectionDebug, MeteorEvent, FfSavedEvent, StarsEvent, SolverStatus } from '$lib/types';
	import { draw } from 'svelte/transition';
	import { zoom as d3Zoom, zoomIdentity } from 'd3-zoom';
	import { select } from 'd3-selection';
	import { t } from '$lib/i18n';
	import { projectConstellations, makeCamera, projectGrid } from '$lib/constellations';
	import { solverLine, fmtRa, fmtDec } from '$lib/solver';

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
	let showConstellations = $state(false);
	let showGrid = $state(false);
	let showCursor = $state(false);
	let detectionDebug = $state<DetectionDebug | null>(null);
	let starsEvent = $state<StarsEvent | null>(null);
	let solverStatus = $state<SolverStatus | null>(null);
	// Zoom of the stage (d3-zoom): scale and pan offset in the overlays' 640×360
	// viewBox units. Declared before the derived overlay values that read them.
	let zoomK = $state(1);
	let zoomX = $state(0);
	let zoomY = $state(0);

	const solverState = $derived(solverLine(solverStatus));
	// Attitude for the sky overlays. The locked reference (sent every block in
	// solver_status, even while the solver is idle) wins; the latest live solve
	// is only the fallback while no reference exists yet.
	const skyPose = $derived(
		solverStatus?.pose
			?? (starsEvent?.solve
				? {
						ra: starsEvent.solve.ra, dec: starsEvent.solve.dec,
						roll: starsEvent.solve.roll, lens: starsEvent.solve.lens ?? { f: 344 },
					}
				: null),
	);
	const skyCamera = $derived(
		skyPose ? makeCamera(skyPose.ra, skyPose.dec, skyPose.roll, skyPose.lens) : null,
	);
	let constellations = $derived(
		skyPose ? projectConstellations(skyPose.ra, skyPose.dec, skyPose.roll, skyPose.lens) : [],
	);
	// Visible part of the frame under the current zoom/pan; the grid re-picks its
	// spacing and label precision from it, so zooming in refines the ticks.
	const gridLines = $derived(
		skyCamera
			? projectGrid(skyCamera, {
					x0: -zoomX / zoomK, y0: -zoomY / zoomK,
					x1: (640 - zoomX) / zoomK, y1: (360 - zoomY) / zoomK,
				})
			: [],
	);

	// Cursor readout: frame pixel + RA/Dec under the pointer.
	let cursor = $state<{ x: number; y: number; ra: number; dec: number } | null>(null);
	function onStagePointer(ev: PointerEvent) {
		if (!showCursor || !stageEl) { cursor = null; return; }
		const r = stageEl.getBoundingClientRect();
		if (r.width === 0 || r.height === 0) return;
		// Stage → overlay viewBox units, then undo the current zoom/pan.
		const vx = ((ev.clientX - r.left) / r.width) * 640;
		const vy = ((ev.clientY - r.top) / r.height) * 360;
		const x = (vx - zoomX) / zoomK;
		const y = (vy - zoomY) / zoomK;
		if (x < 0 || y < 0 || x > 640 || y > 360) { cursor = null; return; }
		const [ra, dec] = skyCamera ? skyCamera.pxToSky(x, y) : [NaN, NaN];
		cursor = { x, y, ra, dec };
	}
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
	let overlayTransform = $state('');
	let bandwidth = $state(0);
	let histZoomK = 1;
	let histZoomX = 0;
	let histLog = $state(false);

	let histogram = new Array(256).fill(0);
    let canvas;
    let ctx;
    let observer;

    let width = 0;
    let height = 0;

	const MAX_METEOR_LOG = 500;
	let meteorLog = $state<MeteorEvent[]>([]);

	const MAX_FF_LOG = 500;
	let ffLog = $state<FfSavedEvent[]>([]);

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
			// History replay on (re)connect may overlap with live events.
			if (meteorLog.some((m) => m.id === e.id && m.ts === e.ts)) return;
			meteorLog = [e, ...meteorLog.slice(0, MAX_METEOR_LOG - 1)];
		};
		ws.onMeteorEnd = (e) => {
			// Final event with full trajectory — replace the confirmation-time entry
			meteorLog = meteorLog.map((m) => (m.id === e.id ? e : m));
		};
		ws.onFfSaved = (e) => {
			if (ffLog.some((f) => f.path === e.path)) return;
			ffLog = [e, ...ffLog.slice(0, MAX_FF_LOG - 1)];
		};
		ws.onStars = (e) => { starsEvent = e; };
		ws.onSolverStatus = (e) => { solverStatus = e; };
		ws.connect();

		// d3-zoom on stage
		if (stageEl) {
			const el = stageEl;

			const zoomBehavior = d3Zoom()
				.scaleExtent([1, 40])
				.on('zoom', (event: any) => {
					const t = event.transform; // mutate: d3 stores this same object
					const k = t.k;

					// Constrain pan so content always covers the viewport
					const w = el.clientWidth;
					const h = el.clientHeight;
					t.x = Math.min(0, Math.max(t.x, w - w * k));
					const cx = t.x;
					t.y = Math.min(0, Math.max(t.y, h - h * k));
					const cy = t.y;
					zoomTransform = `translate(${cx}px,${cy}px) scale(${k})`;
					// Same transform in the overlays' 640×360 viewBox units.
					zoomK = k;
					zoomX = (cx * 640) / w;
					zoomY = (cy * 360) / h;
					overlayTransform = `translate(${(cx * 640) / w} ${(cy * 360) / h}) scale(${k})`;
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

	function sendSchedule(type: string, start: number, end: number) {
		ws?.send({ type, start, end });
	}

	// Hour selector: start ∈ 0..23, end ∈ start+1..start+24 (>24 = next day).
	const startHourOptions = Array.from({ length: 24 }, (_, i) => i);
	function endHourOptions(start: number): number[] {
		return Array.from({ length: 24 }, (_, i) => start + 1 + i);
	}
	function hourLabel(h: number): string {
		if (h < 24) return `${h}:00`;
		return `${h - 24}:00 (+1)`;
	}
	function endDisplay(start: number, end: number): number {
		return end <= start ? end + 24 : end;
	}
	type DnMode = 'auto' | 'day' | 'night';
	function setDaynightMode(mode: DnMode) {
		if (!appState) return;
		if (mode === 'auto') {
			appState.auto_daynight = true;
			send('set_auto_daynight', true);
		} else {
			const night = mode === 'night';
			appState.auto_daynight = false;
			appState.night_mode = night;
			send('set_night_mode', night);
		}
	}

	type IrMode = 'auto' | 'on' | 'off';
	function setIrMode(kind: 'ircut' | 'irled', mode: IrMode) {
		if (!appState) return;
		if (mode === 'auto') {
			if (kind === 'ircut') { appState.auto_ircut = true; send('set_auto_ircut', true); }
			else                  { appState.auto_irled = true; send('set_auto_irled', true); }
		} else {
			const on = mode === 'on';
			if (kind === 'ircut') {
				appState.auto_ircut = false;
				appState.ircut_on = on;
				send('set_ircut', on);
			} else {
				appState.auto_irled = false;
				appState.irled_on = on;
				send('set_irled', on);
			}
		}
	}

	type TwilightMode = 'hours' | 'civil' | 'astro';
	function setTwilightMode(prefix: 'record' | 'timelapse' | 'detection', mode: TwilightMode) {
		if (!appState) return;
		const use_tw = mode !== 'hours';
		const astro  = mode === 'astro';
		(appState as any)[`${prefix}_use_twilight`]   = use_tw;
		(appState as any)[`${prefix}_twilight_astro`] = astro;
		send(`set_${prefix}_use_twilight`, use_tw);
		send(`set_${prefix}_twilight_astro`, astro);
	}

	function clampEnd(start: number, end: number): number {
		const d = endDisplay(start, end);
		if (d <= start) return start + 1;
		if (d > start + 24) return start + 24;
		return d;
	}

	function confirmReboot() {
		if (confirm(t('Reboot the device?'))) {
			ws?.send({ type: 'reboot' });
		}
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
	<div
		class="stage"
		bind:this={stageEl}
		role="presentation"
		onpointermove={onStagePointer}
		onpointerdown={onStagePointer}
		onpointerleave={() => { cursor = null; }}
	>
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
				<span>IT {(ispExprUs / 1000).toFixed(1)}ms</span>
				<span>AG {(ispAgain / 10).toFixed(1)}dB</span>
				<span>AGi {ispAgi}</span>
				<span>DG {(ispDgain / 10).toFixed(1)}dB</span>
				<span>DGi {ispiDgi}</span>
				<span>sDG {(ispsDgain / 10).toFixed(1)}dB</span>
				<span>ITMAX {(ispItMax / 1000).toFixed(1)}ms</span>
				<span>AGMAX {(ispAgMax / 10).toFixed(1)}dB</span>
				<span>iDGMAX {(ispiDgMax / 10).toFixed(1)}dB</span>
				<span>sDGMAX {(ispsDgMax / 10).toFixed(1)}dB</span>
				<span>ITMIN {(ispItMin / 1000).toFixed(1)}ms</span>
				<span>AGMIN {(ispAgMin / 10).toFixed(1)}dB</span>
				<span>iDGMIN {(ispiDgMin / 10).toFixed(1)}dB</span>
				<span>sDGMIN {(ispsDgMin / 10).toFixed(1)}dB</span>
			</div>
		{/if}

		{#if showGrid}
			<svg class="det-overlay" viewBox="0 0 640 360" preserveAspectRatio="none">
				<g transform={overlayTransform}>
					{#each gridLines as g}
						{#each g.paths as path}
							<polyline points={path.map((q) => `${q[0].toFixed(1)},${q[1].toFixed(1)}`).join(' ')} fill="none" stroke={g.kind === 'ra' ? '#fa6' : '#8d8'} stroke-width="1" vector-effect="non-scaling-stroke" stroke-dasharray="3 3" opacity="0.55" />
						{/each}
						{#if g.at}
							<text x={g.at[0] + (g.kind === 'dec' ? 3 : 0) / zoomK} y={g.at[1] - 2 / zoomK} text-anchor={g.kind === 'ra' ? 'middle' : 'start'} fill={g.kind === 'ra' ? '#fa6' : '#8d8'} font-size={9 / zoomK} font-family="monospace" opacity="0.9">{g.label}</text>
						{/if}
					{/each}
				</g>
				{#if !skyCamera}
					<text x="636" y="42" text-anchor="end" fill="#8d8" font-size="11" font-family="monospace">{t('No plate solution')}</text>
				{/if}
			</svg>
		{/if}

		{#if showCursor && cursor}
			<div class="cursor-info">
				<span>x {cursor.x.toFixed(1)} y {cursor.y.toFixed(1)}</span>
				{#if Number.isFinite(cursor.ra)}
					<span>RA {fmtRa(cursor.ra)} ({cursor.ra.toFixed(2)}°)</span>
					<span>Dec {fmtDec(cursor.dec)} ({cursor.dec.toFixed(2)}°)</span>
				{:else}
					<span>{t('No plate solution')}</span>
				{/if}
			</div>
		{/if}

		{#if showConstellations}
			<svg class="det-overlay" viewBox="0 0 640 360" preserveAspectRatio="none">
				<g transform={overlayTransform}>
					{#each constellations as c}
						{#each c.paths as path}
							<polyline points={path.map((q) => `${q[0].toFixed(1)},${q[1].toFixed(1)}`).join(' ')} fill="none" stroke="#4cf" stroke-width="1" vector-effect="non-scaling-stroke" opacity="0.7" />
						{/each}
						{#if c.label}
							<!-- font size / offset divided by zoom so the label stays the same on screen -->
							<text x={c.label[0]} y={c.label[1] - 6 / zoomK} text-anchor="middle" fill="#4cf" font-size={10 / zoomK} font-family="monospace" opacity="0.9">{c.name}</text>
						{/if}
					{/each}
				</g>
				{#if !skyPose}
					<text x="636" y="28" text-anchor="end" fill="#4cf" font-size="11" font-family="monospace">{t('No plate solution')}</text>
				{/if}
			</svg>
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
					<line x1={x0} y1={y0} x2={x1} y2={y1} stroke="#0ff" stroke-width="1.5" opacity="0.85" />
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
    <button class="tab" class:active={activeTab === 'settings'} onclick={() => activeTab = 'settings'}>{t('Settings')}</button>
    <button class="tab" class:active={activeTab === 'log'} onclick={() => activeTab = 'log'}>{t('Log')} {meteorLog.length > 0 ? `(${meteorLog.length})` : ''}</button>
    <button class="tab" class:active={activeTab === 'ff'} onclick={() => activeTab = 'ff'}>FF {ffLog.length > 0 ? `(${ffLog.length})` : ''}</button>
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
				<span class="ctrl-heading">{t('HISTOGRAM')}</span>
				<button class="hist-log-btn" class:active={histLog}
					onclick={() => { histLog = !histLog; draw_hist(histogram); }}>LOG</button>
			</div>
			<canvas bind:this={canvas} class="hist"></canvas>
		</div>

		<!-- Controls -->
		{#if appState}
			<div class="ctrl-group">
				<span class="ctrl-heading">{t('View')}</span>
				<label class="ctrl-row">
					<span>{t('Debug overlay')}</span>
					<input type="checkbox" bind:checked={showDebug} />
				</label>
				<label class="ctrl-row">
					<span>{t('Constellation overlay')}</span>
					<input type="checkbox" bind:checked={showConstellations} />
				</label>
				<label class="ctrl-row">
					<span>{t('Grid overlay')}</span>
					<input type="checkbox" bind:checked={showGrid} />
				</label>
				<label class="ctrl-row">
					<span>{t('Cursor readout')}</span>
					<input type="checkbox" bind:checked={showCursor} />
				</label>
			</div>

			<div class="ctrl-group">
				<span class="ctrl-heading">{t('Camera')}</span>
				<div class="ctrl-row">
					<span>{t('Day / Night')}</span>
					<select class="mode-input"
						onchange={(e) => setDaynightMode(e.currentTarget.value as DnMode)}>
						<option value="auto"  selected={appState.auto_daynight}>{t('Auto')}</option>
						<option value="day"   selected={!appState.auto_daynight && !appState.night_mode}>{t('Day')}</option>
						<option value="night" selected={!appState.auto_daynight && appState.night_mode}>{t('Night')}</option>
					</select>
				</div>
				<div class="ctrl-row">
					<span>{t('Frame rate')}</span>
					<select class="mode-input"
						onchange={(e) => {
							const v = Number(e.currentTarget.value);
							if (appState) appState.fps = v;
							send('set_fps', v);
						}}>
						<option value="25" selected={appState.fps === 25}>25 fps</option>
						<option value="15" selected={appState.fps === 15}>15 fps ({t('low light')})</option>
						{#if appState.fps !== 25 && appState.fps !== 15}
							<option value={appState.fps} selected>{appState.fps} fps</option>
						{/if}
					</select>
				</div>
				<label class="ctrl-row">
					<span>{t('LED')}</span>
					<input type="checkbox" checked={appState.led_on}
						onchange={(e) => send('set_led', e.currentTarget.checked)} />
				</label>
				<div class="ctrl-row">
					<span>{t('IR LED')}</span>
					<select class="mode-input"
						onchange={(e) => setIrMode('irled', e.currentTarget.value as IrMode)}>
						<option value="auto" selected={appState.auto_irled}>{t('Auto')}</option>
						<option value="on"   selected={!appState.auto_irled && appState.irled_on}>{t('On')}</option>
						<option value="off"  selected={!appState.auto_irled && !appState.irled_on}>{t('Off')}</option>
					</select>
				</div>
				<div class="ctrl-row">
					<span>{t('IR Cut')}</span>
					<select class="mode-input"
						onchange={(e) => setIrMode('ircut', e.currentTarget.value as IrMode)}>
						<option value="auto" selected={appState.auto_ircut}>{t('Auto')}</option>
						<option value="on"   selected={!appState.auto_ircut && appState.ircut_on}>{t('On')}</option>
						<option value="off"  selected={!appState.auto_ircut && !appState.ircut_on}>{t('Off')}</option>
					</select>
				</div>

			</div>

			<div class="ctrl-group">
				<span class="ctrl-heading">{t('Functions')}</span>
				{#if (appState.record_use_twilight || appState.timelapse_use_twilight || appState.detection_use_twilight || appState.solver_use_twilight)
					&& Math.abs(appState.latitude - 35.6812) < 0.0001
					&& Math.abs(appState.longitude - 139.7671) < 0.0001}
					<a class="coord-warning" href="/settings">{t('⚠ Location is default (Tokyo). Configure in Settings.')}</a>
				{/if}
				<label class="ctrl-row">
					<span>{t('Recording')}</span>
					<input type="checkbox" checked={appState.record_enabled}
						onchange={(e) => send('set_record', e.currentTarget.checked)} />
				</label>
				<div class="schedule-row">
					<select class="mode-input"
						onchange={(e) => setTwilightMode('record', e.currentTarget.value as TwilightMode)}>
						<option value="hours" selected={!appState.record_use_twilight}>{t('Hours')}</option>
						<option value="civil" selected={appState.record_use_twilight && !appState.record_twilight_astro}>{t('Civil')}</option>
						<option value="astro" selected={appState.record_use_twilight && appState.record_twilight_astro}>{t('Astro')}</option>
					</select>
					{#if !appState.record_use_twilight}
						<select class="hour-input"
							onchange={(e) => { const s = +e.currentTarget.value; appState!.record_start_hour = s; appState!.record_end_hour = clampEnd(s, appState!.record_end_hour); sendSchedule('set_record_schedule', appState!.record_start_hour, appState!.record_end_hour); }}>
							{#each startHourOptions as h}
								<option value={h} selected={appState.record_start_hour === h}>{hourLabel(h)}</option>
							{/each}
						</select>
						<span class="hour-sep">–</span>
						<select class="hour-input"
							onchange={(e) => { appState!.record_end_hour = +e.currentTarget.value; sendSchedule('set_record_schedule', appState!.record_start_hour, appState!.record_end_hour); }}>
							{#each endHourOptions(appState.record_start_hour) as h}
								<option value={h} selected={endDisplay(appState.record_start_hour, appState.record_end_hour) === h}>{hourLabel(h)}</option>
							{/each}
						</select>
					{/if}
				</div>
				<label class="ctrl-row">
					<span>{t('Timelapse')}</span>
					<input type="checkbox" checked={appState.timelapse_enabled}
						onchange={(e) => send('set_timelapse', e.currentTarget.checked)} />
				</label>
				<div class="schedule-row">
					<select class="mode-input"
						onchange={(e) => setTwilightMode('timelapse', e.currentTarget.value as TwilightMode)}>
						<option value="hours" selected={!appState.timelapse_use_twilight}>{t('Hours')}</option>
						<option value="civil" selected={appState.timelapse_use_twilight && !appState.timelapse_twilight_astro}>{t('Civil')}</option>
						<option value="astro" selected={appState.timelapse_use_twilight && appState.timelapse_twilight_astro}>{t('Astro')}</option>
					</select>
					{#if !appState.timelapse_use_twilight}
						<select class="hour-input"
							onchange={(e) => { const s = +e.currentTarget.value; appState!.timelapse_start_hour = s; appState!.timelapse_end_hour = clampEnd(s, appState!.timelapse_end_hour); sendSchedule('set_timelapse_schedule', appState!.timelapse_start_hour, appState!.timelapse_end_hour); }}>
							{#each startHourOptions as h}
								<option value={h} selected={appState.timelapse_start_hour === h}>{hourLabel(h)}</option>
							{/each}
						</select>
						<span class="hour-sep">–</span>
						<select class="hour-input"
							onchange={(e) => { appState!.timelapse_end_hour = +e.currentTarget.value; sendSchedule('set_timelapse_schedule', appState!.timelapse_start_hour, appState!.timelapse_end_hour); }}>
							{#each endHourOptions(appState.timelapse_start_hour) as h}
								<option value={h} selected={endDisplay(appState.timelapse_start_hour, appState.timelapse_end_hour) === h}>{hourLabel(h)}</option>
							{/each}
						</select>
					{/if}
				</div>
				<label class="ctrl-row">
					<span>{t('Detection')}</span>
					<input type="checkbox" checked={appState.detection_enabled}
						onchange={(e) => send('set_detection', e.currentTarget.checked)} />
				</label>
				<div class="schedule-row">
					<select class="mode-input"
						onchange={(e) => setTwilightMode('detection', e.currentTarget.value as TwilightMode)}>
						<option value="hours" selected={!appState.detection_use_twilight}>{t('Hours')}</option>
						<option value="civil" selected={appState.detection_use_twilight && !appState.detection_twilight_astro}>{t('Civil')}</option>
						<option value="astro" selected={appState.detection_use_twilight && appState.detection_twilight_astro}>{t('Astro')}</option>
					</select>
					{#if !appState.detection_use_twilight}
						<select class="hour-input"
							onchange={(e) => { const s = +e.currentTarget.value; appState!.detection_start_hour = s; appState!.detection_end_hour = clampEnd(s, appState!.detection_end_hour); sendSchedule('set_detection_schedule', appState!.detection_start_hour, appState!.detection_end_hour); }}>
							{#each startHourOptions as h}
								<option value={h} selected={appState.detection_start_hour === h}>{hourLabel(h)}</option>
							{/each}
						</select>
						<span class="hour-sep">–</span>
						<select class="hour-input"
							onchange={(e) => { appState!.detection_end_hour = +e.currentTarget.value; sendSchedule('set_detection_schedule', appState!.detection_start_hour, appState!.detection_end_hour); }}>
							{#each endHourOptions(appState.detection_start_hour) as h}
								<option value={h} selected={endDisplay(appState.detection_start_hour, appState.detection_end_hour) === h}>{hourLabel(h)}</option>
							{/each}
						</select>
					{/if}
				</div>
				<label class="ctrl-row">
					<span>{t('Detection Record')}</span>
					<input type="checkbox" checked={appState.detection_record_enabled}
						onchange={(e) => send('set_detection_record', e.currentTarget.checked)} />
				</label>
				<label class="ctrl-row">
					<span>{t('Plate Solver')}</span>
					<input type="checkbox" checked={appState.solver_enabled}
						onchange={(e) => { appState!.solver_enabled = e.currentTarget.checked; send('set_solver', e.currentTarget.checked); }} />
				</label>
				{#if solverState[0]}
					<span class="solver-status {solverState[1]}">{solverState[0]}</span>
				{/if}
			</div>

			<div class="ctrl-group">
				<span class="ctrl-heading">{t('OSD')}</span>
				<label class="ctrl-row">
					<span>{t('Timestamp')}</span>
					<input type="checkbox" checked={appState.show_timestamp}
						onchange={(e) => send('set_show_timestamp', e.currentTarget.checked)} />
				</label>
				<div class="ctrl-row">
					<span>{t('Position')}</span>
					<select class="select"
						value={appState.timestamp_position}
						onchange={(e) => send('set_timestamp_position', +e.currentTarget.value)}>
						<option value={0}>{t('Top-left')}</option>
						<option value={1}>{t('Top-right')}</option>
						<option value={2}>{t('Bottom-left')}</option>
						<option value={3}>{t('Bottom-right')}</option>
					</select>
				</div>
			</div>

			<div class="ctrl-group">
				<span class="ctrl-heading">{t('Webhooks')}</span>
				<div class="text-row">
					<span class="text-label">Discord</span>
					<input type="text" class="url-input" placeholder="https://..."
						value={appState.webhook_discord_url}
						onchange={(e) => send('set_webhook_discord', e.currentTarget.value)} />
				</div>
				<div class="text-row">
					<span class="text-label">Slack</span>
					<input type="text" class="url-input" placeholder="https://..."
						value={appState.webhook_slack_url}
						onchange={(e) => send('set_webhook_slack', e.currentTarget.value)} />
				</div>
				<div class="text-row">
					<span class="text-label">Generic</span>
					<input type="text" class="url-input" placeholder="https://..."
						value={appState.webhook_generic_url}
						onchange={(e) => send('set_webhook_generic', e.currentTarget.value)} />
				</div>
				<label class="ctrl-row">
					<span>{t('Notify on startup')}</span>
					<input type="checkbox" checked={appState.webhook_notify_startup}
						onchange={(e) => send('set_webhook_notify_startup', e.currentTarget.checked)} />
				</label>
				<label class="ctrl-row">
					<span>{t('Notify detection start')}</span>
					<input type="checkbox" checked={appState.webhook_notify_detection_start}
						onchange={(e) => send('set_webhook_notify_detection_start', e.currentTarget.checked)} />
				</label>
				<label class="ctrl-row">
					<span>{t('Notify detection end')}</span>
					<input type="checkbox" checked={appState.webhook_notify_detection_end}
						onchange={(e) => send('set_webhook_notify_detection_end', e.currentTarget.checked)} />
				</label>
			</div>

			<div class="ctrl-group">
				<span class="ctrl-heading">{t('System')}</span>
				<div class="ctrl-row">
					<span>{t('Capture FITS')}</span>
					<button class="action-btn" onclick={() => ws?.send({ type: 'capture_fits' })}>{t('Capture')}</button>
				</div>
				<div class="ctrl-row">
					<span>{t('Reboot')}</span>
					<button class="action-btn danger" onclick={confirmReboot}>{t('Reboot')}</button>
				</div>
			</div>
		{/if}

	</div>
			<!-- ── Detection log ───────────────── -->
	<div style={`display:${activeTab === 'log' ? "flex" : "none"}`}  class="settings-bar">
	{#if meteorLog.length > 0}
		<div class="meteor-log">
			<div class="log-header">
				<span class="log-title">{t('Meteors')}</span>
				<span class="log-count">{meteorLog.length}</span>
				<button class="log-clear" onclick={() => { meteorLog = []; }}>{t('clear')}</button>
			</div>
			{#each meteorLog as m}
				<div class="log-entry">
					<span class="log-ts">{m.ts.slice(9, 11)}:{m.ts.slice(11, 13)}:{m.ts.slice(13, 15)}</span>
					<span class="log-id">#{m.id}</span>
					{#if m.ff}<span class="log-tag">FF</span>{:else if m.short}<span class="log-tag">S</span>{/if}
					<span class="log-speed">{m.speed.toFixed(1)} px/f</span>
					<span class="log-dur">{m.frames} fr</span>
					<a class="log-img" href="/files/detections/{m.ts}.png" target="_blank" rel="noopener noreferrer" title={t('View stack image')}>
						<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><polyline points="21 15 16 10 5 21"/>
						</svg>
					</a>
				</div>
			{/each}
		</div>
	{:else}
		<div class="log-empty">{t('No detections yet')}</div>
	{/if}
		</div>

	<!-- ── FF log ───────────────── -->
	<div style={`display:${activeTab === 'ff' ? "flex" : "none"}`} class="settings-bar">
		<div class="meteor-log">
			<div class="log-header">
				<span class="log-title" style="color:#8be">{t('FF Blocks')}</span>
				<span class="log-count">{ffLog.length}</span>
				<button class="log-clear" onclick={() => { ffLog = []; }}>{t('clear')}</button>
			</div>
			{#if ffLog.length === 0}
				<div class="log-empty">{t('No FF blocks saved yet')}</div>
			{:else}
				{#each ffLog as f}
					<div class="log-entry">
						<span class="log-ts">{f.ts.slice(9, 11)}:{f.ts.slice(11, 13)}:{f.ts.slice(13, 15)}</span>
						<span class="ff-reason ff-reason--{f.reason}">{f.reason}</span>
						<a class="log-img" href={f.path} download title={t('Download FITS')}>
							<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
								<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/>
							</svg>
						</a>
					</div>
				{/each}
			{/if}
		</div>
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

	.cursor-info {
		position: absolute;
		top: 2.2rem;
		left: 0.5rem;
		display: flex;
		flex-wrap: wrap;
		gap: 0.6rem;
		font-size: 0.7rem;
		font-family: monospace;
		color: #fff;
		background: rgba(0, 0, 0, 0.6);
		padding: 0.15rem 0.5rem;
		border-radius: 4px;
		pointer-events: none;
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

	.schedule-row {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		padding: 0.3rem 0;
		border-top: 1px solid var(--border2);
	}

	.hour-input {
		width: 3rem;
		padding: 0.15rem 0.3rem;
		font-family: monospace;
		font-size: 0.75rem;
		background: var(--input);
		color: var(--text);
		border: 1px solid var(--border);
		border-radius: 3px;
	}

	.mode-input {
		flex: 1;
		min-width: 0;
		padding: 0.15rem 0.3rem;
		font-size: 0.7rem;
		background: var(--input);
		color: var(--text);
		border: 1px solid var(--border);
		border-radius: 3px;
	}

	.coord-warning {
		display: block;
		font-size: 0.7rem;
		line-height: 1.3;
		padding: 0.3rem 0.4rem;
		margin: 0.2rem 0 0.1rem;
		background: #3a2a0a;
		color: #f90;
		border-radius: 4px;
		text-decoration: none;
	}
	.coord-warning:hover { background: #4a3510; }

	.solver-status {
		display: block;
		font-size: 0.7rem;
		line-height: 1.3;
		padding: 0.15rem 0.4rem 0.3rem;
		color: var(--text3);
	}
	.solver-status.ok { color: #4caf78; }
	.solver-status.work { color: #6aa9ff; }
	.solver-status.warn { color: #f90; }

	.hour-sep {
		color: var(--text3);
		font-size: 0.75rem;
	}

	.text-row {
		display: flex;
		flex-direction: column;
		gap: 0.2rem;
		padding: 0.3rem 0;
		border-top: 1px solid var(--border2);
	}
	.text-row:first-of-type { border-top: none; }

	.text-label {
		font-size: 0.7rem;
		font-weight: 600;
		color: var(--text3);
	}

	.url-input {
		width: 100%;
		padding: 0.2rem 0.35rem;
		font-family: monospace;
		font-size: 0.7rem;
		background: var(--input);
		color: var(--text);
		border: 1px solid var(--border);
		border-radius: 3px;
		box-sizing: border-box;
	}

	.select {
		padding: 0.15rem 0.3rem;
		font-size: 0.75rem;
		background: var(--input);
		color: var(--text);
		border: 1px solid var(--border);
		border-radius: 3px;
	}

	.action-btn {
		padding: 0.2rem 0.6rem;
		font-size: 0.75rem;
		background: var(--input);
		color: var(--text);
		border: 1px solid var(--border);
		border-radius: 4px;
		cursor: pointer;
	}
	.action-btn:hover { border-color: var(--text3); }
	.action-btn.danger {
		color: #e74c3c;
		border-color: #6e2a23;
	}
	.action-btn.danger:hover {
		background: #3a1a16;
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
	.log-tag   {
		color: #8be;
		background: #1a3a5e;
		border-radius: 3px;
		padding: 0 0.25rem;
		font-size: 0.65rem;
		flex-shrink: 0;
	}
	.log-speed { color: #f80;         flex-shrink: 0; }
	.log-dur   { color: var(--text3); flex-shrink: 0; }
	.log-img   { color: var(--text3); flex-shrink: 0; display: flex; align-items: center; margin-left: auto; }
	.log-img:hover { color: var(--text); }

	.log-empty {
		padding: 0.5rem 0;
		color: var(--text4);
		font-size: 0.75rem;
		text-align: center;
	}

	.ff-reason {
		font-size: 0.7rem;
		font-weight: 600;
		padding: 0.05rem 0.35rem;
		border-radius: 3px;
		flex-shrink: 0;
	}
	.ff-reason--detected    { background: #1a3a5e; color: #5bc; }
	.ff-reason--interesting { background: #3a2a0a; color: #f90; }
	.ff-reason--baseline    { background: #1a2a1a; color: #6a6; }
</style>
