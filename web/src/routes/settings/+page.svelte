<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { AtometWebSocket } from '$lib/ws';
	import type { AppState, SolverStatus } from '$lib/types';
	import { t } from '$lib/i18n';
	import { solverLine, fmtRa, fmtDec } from '$lib/solver';

	let ws: AtometWebSocket;
	let appState = $state<AppState | null>(null);
	let wsConnected = $state(false);

	// Reboot inside the detection period loses that night's log. Exact for a
	// fixed-hour schedule; twilight schedules vary by date, so warn when enabled.
	const rebootOverlapsDetection = $derived.by(() => {
		const a = appState;
		if (!a?.scheduled_reboot_enabled || !a.detection_enabled) return false;
		if (a.detection_use_twilight) return true;
		const h = a.scheduled_reboot_hour;
		const start = a.detection_start_hour;
		const end = a.detection_end_hour > 24 ? a.detection_end_hour - 24 : a.detection_end_hour;
		return start <= end ? h >= start && h < end : h >= start || h < end;
	});

	// ── Plate-solver calibration card ───────────────────────────────────────
	let solverStatus = $state<SolverStatus | null>(null);
	const solverState = $derived(solverLine(solverStatus));
	const calibPct = $derived(
		solverStatus && solverStatus.need > 0
			? Math.min(100, Math.round((solverStatus.samples / solverStatus.need) * 100))
			: 0,
	);
	// Two-step confirm without a native dialog: the first click arms the button,
	// the second one sends. It disarms itself after a few seconds.
	let recalArmed = $state(false);
	let recalNote = $state(false);
	let recalTimer: ReturnType<typeof setTimeout> | undefined;
	let noteTimer: ReturnType<typeof setTimeout> | undefined;

	function armRecalibrate() {
		recalArmed = true;
		clearTimeout(recalTimer);
		recalTimer = setTimeout(() => { recalArmed = false; }, 6000);
	}
	function cancelRecalibrate() {
		recalArmed = false;
		clearTimeout(recalTimer);
	}
	function doRecalibrate() {
		cancelRecalibrate();
		send('solver_recalibrate', true);
		recalNote = true;
		clearTimeout(noteTimer);
		noteTimer = setTimeout(() => { recalNote = false; }, 10000);
	}

	type Tab = 'system' | 'detection' | 'record' | 'image' | 'webhook';
	let activeTab = $state<Tab>('system');

	const tabs: { id: Tab; label: string }[] = [
		{ id: 'system',    label: t('System')    },
		{ id: 'detection', label: t('Detection') },
		{ id: 'record',    label: t('Record')    },
		{ id: 'image',     label: t('Image')     },
		{ id: 'webhook',   label: t('Webhook')   },
	];

	// ── ISP bypass (image processing toggles) ───────────────────────────────
	// The kernel exposes a 32-element u32 array; bit value 1 = bypass (feature off),
	// 0 = active (feature on). We mirror the array locally and re-send the whole
	// array on every toggle.
	let bypassBits = $state<number[] | null>(null);

	type IspToggle = {
		bit: number;
		key: string;       // i18n key
		desc: string;      // i18n key for tooltip / description
		meteorHint?: 'off' | 'on';  // recommended setting for meteor observation
	};
	const ispToggles: IspToggle[] = [
		{ bit: 2,  key: 'DPC (Dead Pixel Correction)',     desc: 'isp_desc_dpc' },
		{ bit: 7,  key: 'ADR (Adaptive Dynamic Range)',    desc: 'isp_desc_adr',     meteorHint: 'off' },
		{ bit: 10, key: 'Gamma',                            desc: 'isp_desc_gamma' },
		{ bit: 11, key: 'Defog',                            desc: 'isp_desc_defog',   meteorHint: 'off' },
		{ bit: 12, key: 'CLM / BCSH (color)',               desc: 'isp_desc_clm' },
		{ bit: 14, key: 'Sharpen',                          desc: 'isp_desc_sharpen', meteorHint: 'off' },
		{ bit: 15, key: 'SDNS (Spatial Denoise)',           desc: 'isp_desc_sdns' },
		{ bit: 16, key: 'MDNS (Motion Denoise)',            desc: 'isp_desc_mdns',    meteorHint: 'off' },
		{ bit: 17, key: 'YDNS (Y Denoise)',                 desc: 'isp_desc_ydns' },
	];

	function isFeatureOn(bit: number): boolean {
		if (!bypassBits) return false;
		return bypassBits[bit] === 0;
	}

	function setFeature(bit: number, on: boolean) {
		if (!bypassBits) return;
		const next = bypassBits.slice();
		next[bit] = on ? 0 : 1;
		bypassBits = next;
		ws?.send({ type: 'set_bypass', bits: next });
	}

	function fmtExposure(us: number): string {
		if (us === 0) return 'Auto';
		if (us < 1000) return `${us}μs`;
		return `${(us / 1000).toFixed(1)}ms`;
	}

	function fmtGain(g: number): string {
		if (g === 0) return 'Auto';
		return `${(g / 1024).toFixed(2)}×`;
	}

	onMount(() => {
		ws = new AtometWebSocket();
		ws.onStatusChange = (c) => { wsConnected = c; };
		ws.onAppState     = (s) => { appState = s; };
		ws.onBypass       = (bits) => { bypassBits = bits; };
		ws.onSolverStatus = (s) => { solverStatus = s; };
		ws.connect();
	});

	// Refresh bypass state whenever the user opens the Image tab.
	// Prefer the persisted bits from AppState; only fall back to `get_bypass`
	// (kernel factory default) when nothing has been persisted yet.
	$effect(() => {
		if (activeTab !== 'image' || !wsConnected) return;
		if (appState && appState.isp_bypass && appState.isp_bypass.length === 32) {
			bypassBits = appState.isp_bypass.slice();
		} else {
			ws?.send({ type: 'get_bypass' });
		}
	});

	onDestroy(() => {
		clearTimeout(recalTimer);
		clearTimeout(noteTimer);
		ws?.disconnect();
	});

	function send(type: string, value: unknown) {
		ws?.send({ type, value });
	}

	function sendSchedule(type: string, start: number, end: number) {
		ws?.send({ type, start, end });
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
	function setTwilightMode(prefix: 'record' | 'timelapse' | 'detection' | 'solver', mode: TwilightMode) {
		if (!appState) return;
		const use_tw = mode !== 'hours';
		const astro  = mode === 'astro';
		(appState as any)[`${prefix}_use_twilight`]   = use_tw;
		(appState as any)[`${prefix}_twilight_astro`] = astro;
		send(`set_${prefix}_use_twilight`, use_tw);
		send(`set_${prefix}_twilight_astro`, astro);
	}

	function hourLabel(h: number): string {
		if (h < 24) return `${h}:00`;
		return `${h - 24}:00 (+1)`;
	}

	// Start: 0..23 (today). End: start+1..start+24 (must be after start, ≥+1day).
	const startHourOptions = Array.from({ length: 24 }, (_, i) => i);
	function endHourOptions(start: number): number[] {
		return Array.from({ length: 24 }, (_, i) => start + 1 + i);
	}
	// Normalize end for display: if end<=start (legacy wrap), treat as end+24.
	function endDisplay(start: number, end: number): number {
		return end <= start ? end + 24 : end;
	}
	// Re-clamp end when start changes so end stays in (start, start+24].
	function clampEnd(start: number, end: number): number {
		const d = endDisplay(start, end);
		if (d <= start) return start + 1;
		if (d > start + 24) return start + 24;
		return d;
	}

	// POSIX TZ strings — work without /usr/share/zoneinfo.
	const timezones: { value: string; label: string }[] = [
		{ value: 'JST-9',                                  label: 'Japan (JST)' },
		{ value: 'KST-9',                                  label: 'Korea (KST)' },
		{ value: 'CST-8',                                  label: 'China (CST)' },
		{ value: 'IST-5:30',                               label: 'India (IST)' },
		{ value: 'AEST-10AEDT,M10.1.0,M4.1.0/3',           label: 'Australia Sydney' },
		{ value: 'UTC0',                                   label: 'UTC' },
		{ value: 'GMT0BST,M3.5.0/1,M10.5.0',               label: 'UK (GMT/BST)' },
		{ value: 'CET-1CEST,M3.5.0,M10.5.0/3',             label: 'Europe Central (CET/CEST)' },
		{ value: 'EST5EDT,M3.2.0,M11.1.0',                 label: 'US Eastern' },
		{ value: 'CST6CDT,M3.2.0,M11.1.0',                 label: 'US Central' },
		{ value: 'MST7MDT,M3.2.0,M11.1.0',                 label: 'US Mountain' },
		{ value: 'PST8PDT,M3.2.0,M11.1.0',                 label: 'US Pacific' },
		{ value: 'HST10',                                  label: 'Hawaii' },
	];
</script>

<div class="page">
	<!-- Page header -->
	<div class="page-header">
		<h1>{t('Settings')}</h1>
		<span class="ws-pill" class:on={wsConnected}>
			<span class="ws-dot"></span>
			{wsConnected ? t('Connected') : t('Disconnected')}
		</span>
	</div>

	<!-- Tabs (left-aligned) -->
	<div class="tabs" role="tablist">
		{#each tabs as tab}
			<button
				role="tab"
				class="tab"
				class:active={activeTab === tab.id}
				aria-selected={activeTab === tab.id}
				onclick={() => { activeTab = tab.id; }}
			>
				{tab.label}
			</button>
		{/each}
	</div>

	<!-- Tab content -->
	{#if !appState}
		<div class="empty">
			<span class="spinner"></span>
			{t('Connecting…')}
		</div>
	{:else}
		<div class="content" role="tabpanel">

			<!-- ── System ─────────────────────────────────── -->
			{#if activeTab === 'system'}
				<section>
					<h2>{t('Camera')}</h2>
					<div class="row">
						<span>{t('Day / Night')}</span>
						<div class="select-wrap">
							<select onchange={(e) => setDaynightMode(e.currentTarget.value as DnMode)}>
								<option value="auto"  selected={appState.auto_daynight}>{t('Auto')}</option>
								<option value="day"   selected={!appState.auto_daynight && !appState.night_mode}>{t('Day')}</option>
								<option value="night" selected={!appState.auto_daynight && appState.night_mode}>{t('Night')}</option>
							</select>
						</div>
					</div>
					<div class="row">
						<span>{t('IR Cut Filter')}</span>
						<div class="select-wrap">
							<select onchange={(e) => setIrMode('ircut', e.currentTarget.value as IrMode)}>
								<option value="auto" selected={appState.auto_ircut}>{t('Auto')}</option>
								<option value="on"   selected={!appState.auto_ircut && appState.ircut_on}>{t('On')}</option>
								<option value="off"  selected={!appState.auto_ircut && !appState.ircut_on}>{t('Off')}</option>
							</select>
						</div>
					</div>
					<label class="row">
						<span>{t('LED')}</span>
						<input type="checkbox" checked={appState.led_on}
							onchange={(e) => send('set_led', e.currentTarget.checked)} />
					</label>
					<div class="row">
						<span>{t('IR LED')}</span>
						<div class="select-wrap">
							<select onchange={(e) => setIrMode('irled', e.currentTarget.value as IrMode)}>
								<option value="auto" selected={appState.auto_irled}>{t('Auto')}</option>
								<option value="on"   selected={!appState.auto_irled && appState.irled_on}>{t('On')}</option>
								<option value="off"  selected={!appState.auto_irled && !appState.irled_on}>{t('Off')}</option>
							</select>
						</div>
					</div>
				</section>

				<section>
					<h2>{t('Time')}</h2>
					<label class="row">
						<span>{t('Timezone')}</span>
						<div class="select-wrap">
							<select
								onchange={(e) => send('set_timezone', e.currentTarget.value)}
							>
								{#each timezones as tz}
									<option value={tz.value} selected={appState.timezone === tz.value}>{tz.label}</option>
								{/each}
								{#if !timezones.some(t => t.value === appState.timezone)}
									<option value={appState.timezone} selected>{appState.timezone}</option>
								{/if}
							</select>
						</div>
					</label>
				</section>

				<section>
					<h2>{t('Location')}</h2>
					<p class="hint">{t('Used for civil-twilight scheduling. North/East positive.')}</p>
					<label class="row">
						<span>{t('Latitude')}</span>
						<input class="num-input" type="number" min="-90" max="90" step="0.0001"
							value={appState.latitude}
							onchange={(e) => { appState.latitude = +e.currentTarget.value; ws?.send({ type: 'set_location', latitude: appState.latitude, longitude: appState.longitude }); }} />
					</label>
					<label class="row">
						<span>{t('Longitude')}</span>
						<input class="num-input" type="number" min="-180" max="180" step="0.0001"
							value={appState.longitude}
							onchange={(e) => { appState.longitude = +e.currentTarget.value; ws?.send({ type: 'set_location', latitude: appState.latitude, longitude: appState.longitude }); }} />
					</label>
				</section>

				<section>
					<h2>{t('OSD')}</h2>
					<label class="row">
						<span>{t('Show Timestamp')}</span>
						<input type="checkbox" checked={appState.show_timestamp}
							onchange={(e) => send('set_show_timestamp', e.currentTarget.checked)} />
					</label>
				<label class="row">
					<span>{t('Timestamp Position')}</span>
					<div class="select-wrap">
						<select
							onchange={(e) => send('set_timestamp_position', +e.currentTarget.value)}
						>
							<option value={0} selected={appState.timestamp_position === 0}>{t('Top Left')}</option>
							<option value={1} selected={appState.timestamp_position === 1}>{t('Top Right')}</option>
							<option value={2} selected={appState.timestamp_position === 2}>{t('Bottom Left')}</option>
							<option value={3} selected={appState.timestamp_position === 3}>{t('Bottom Right')}</option>
						</select>
					</div>
				</label>
				</section>

				<section>
					<h2>{t('Maintenance')}</h2>
					<label class="row">
						<span>{t('Daily auto-reboot')}</span>
						<input type="checkbox" checked={appState.scheduled_reboot_enabled}
							onchange={(e) => send('set_scheduled_reboot', e.currentTarget.checked)} />
					</label>
					{#if appState.scheduled_reboot_enabled}
						<label class="row">
							<span>{t('Reboot time')}</span>
							<div class="select-wrap" style="display:flex;gap:0.3rem;align-items:center">
								<select onchange={(e) => send('set_scheduled_reboot_hour', +e.currentTarget.value)}>
									{#each Array.from({length: 24}, (_, i) => i) as h}
										<option value={h} selected={appState.scheduled_reboot_hour === h}>{String(h).padStart(2, '0')}</option>
									{/each}
								</select>
								:
								<select onchange={(e) => send('set_scheduled_reboot_minute', +e.currentTarget.value)}>
									{#each Array.from({length: 60}, (_, i) => i) as m}
										<option value={m} selected={(appState.scheduled_reboot_minute ?? 0) === m}>{String(m).padStart(2, '0')}</option>
									{/each}
								</select>
							</div>
						</label>
						<p class="hint">{t('Rebooting restarts the process and resets the detection log. Choose a time outside the detection period.')}</p>
						{#if rebootOverlapsDetection}
							<p class="hint" style="color:#f80">{t('Warning: this time may fall inside the detection period.')}</p>
						{/if}
					{/if}
				</section>

			<!-- ── Detection ─────────────────────────────── -->
			{:else if activeTab === 'detection'}
				<section>
					<h2>{t('Detection')}</h2>
					<label class="row">
						<span>{t('Enable Detection')}</span>
						<input type="checkbox" checked={appState.detection_enabled}
							onchange={(e) => send('set_detection', e.currentTarget.checked)} />
					</label>
					<label class="row">
						<span>{t('Mode')}</span>
						<div class="select-wrap">
							<select onchange={(e) => setTwilightMode('detection', e.currentTarget.value as TwilightMode)}>
								<option value="hours" selected={!appState.detection_use_twilight}>{t('By hours')}</option>
								<option value="civil" selected={appState.detection_use_twilight && !appState.detection_twilight_astro}>{t('Civil twilight (night)')}</option>
								<option value="astro" selected={appState.detection_use_twilight && appState.detection_twilight_astro}>{t('Astronomical twilight (deep night)')}</option>
							</select>
						</div>
					</label>
					{#if !appState.detection_use_twilight}
					<div class="row">
						<span>{t('Schedule')}</span>
						<div class="schedule-wrap">
							<select onchange={(e) => { const s = +e.currentTarget.value; appState.detection_start_hour = s; appState.detection_end_hour = clampEnd(s, appState.detection_end_hour); sendSchedule('set_detection_schedule', appState.detection_start_hour, appState.detection_end_hour); }}>
								{#each startHourOptions as h}
									<option value={h} selected={appState.detection_start_hour === h}>{hourLabel(h)}</option>
								{/each}
							</select>
							<span class="schedule-sep">–</span>
							<select onchange={(e) => { appState.detection_end_hour = +e.currentTarget.value; sendSchedule('set_detection_schedule', appState.detection_start_hour, appState.detection_end_hour); }}>
								{#each endHourOptions(appState.detection_start_hour) as h}
									<option value={h} selected={endDisplay(appState.detection_start_hour, appState.detection_end_hour) === h}>{hourLabel(h)}</option>
								{/each}
							</select>
						</div>
					</div>
					{/if}
				</section>

				<section>
					<h2>{t('Detection Recording')}</h2>
					<label class="row">
						<span>{t('Enable')}</span>
						<input type="checkbox" checked={appState.detection_record_enabled}
							onchange={(e) => send('set_detection_record', e.currentTarget.checked)} />
					</label>
				</section>

				<section>
					<h2>{t('Plate Solver')}</h2>
					<label class="row">
						<span>{t('Enable Plate Solver')}</span>
						<input type="checkbox" checked={appState.solver_enabled}
							onchange={(e) => { appState!.solver_enabled = e.currentTarget.checked; send('set_solver', e.currentTarget.checked); }} />
					</label>
					<label class="row">
						<span>{t('Mode')}</span>
						<div class="select-wrap">
							<select onchange={(e) => setTwilightMode('solver', e.currentTarget.value as TwilightMode)}>
								<option value="hours" selected={!appState.solver_use_twilight}>{t('By hours')}</option>
								<option value="civil" selected={appState.solver_use_twilight && !appState.solver_twilight_astro}>{t('Civil twilight (night)')}</option>
								<option value="astro" selected={appState.solver_use_twilight && appState.solver_twilight_astro}>{t('Astronomical twilight (deep night)')}</option>
							</select>
						</div>
					</label>
					{#if !appState.solver_use_twilight}
					<div class="row">
						<span>{t('Schedule')}</span>
						<div class="schedule-wrap">
							<select onchange={(e) => { const s = +e.currentTarget.value; appState!.solver_start_hour = s; appState!.solver_end_hour = clampEnd(s, appState!.solver_end_hour); sendSchedule('set_solver_schedule', appState!.solver_start_hour, appState!.solver_end_hour); }}>
								{#each startHourOptions as h}
									<option value={h} selected={appState.solver_start_hour === h}>{hourLabel(h)}</option>
								{/each}
							</select>
							<span class="schedule-sep">–</span>
							<select onchange={(e) => { appState!.solver_end_hour = +e.currentTarget.value; sendSchedule('set_solver_schedule', appState!.solver_start_hour, appState!.solver_end_hour); }}>
								{#each endHourOptions(appState.solver_start_hour) as h}
									<option value={h} selected={endDisplay(appState.solver_start_hour, appState.solver_end_hour) === h}>{hourLabel(h)}</option>
								{/each}
							</select>
						</div>
					</div>
					{/if}
				</section>

				<section>
					<h2>{t('Calibration')}</h2>
					<div class="calib {solverState[1]}">
						<div class="calib-head">
							<span class="calib-dot" aria-hidden="true"></span>
							<span class="calib-title">{solverState[0] || t('No calibration data yet')}</span>
						</div>

						{#if solverStatus && !solverStatus.has_ref && solverStatus.samples > 0}
							<div class="calib-bar" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow={calibPct}>
								<div style="width:{calibPct}%"></div>
							</div>
						{/if}

						{#if solverStatus?.pose}
							{@const p = solverStatus.pose}
							<dl class="calib-grid">
								<div><dt>RA</dt><dd>{fmtRa(p.ra)}</dd></div>
								<div><dt>Dec</dt><dd>{fmtDec(p.dec)}</dd></div>
								<div><dt>{t('Roll')}</dt><dd>{p.roll.toFixed(1)}°</dd></div>
								<div><dt>{t('Focal length')}</dt><dd>{p.lens.f.toFixed(0)} px</dd></div>
								<div><dt>{t('Lens model')}</dt><dd>{p.lens.c3 !== undefined ? t('Polynomial') : t('Pinhole')}</dd></div>
							</dl>
						{/if}

						<div class="calib-actions">
							{#if recalArmed}
								<span class="calib-warn">{t('Discard the saved solution and calibrate again?')}</span>
								<button class="btn ghost" onclick={cancelRecalibrate}>{t('Cancel')}</button>
								<button class="btn danger" onclick={doRecalibrate}>{t('Recalibrate')}</button>
							{:else}
								<button class="btn outline" onclick={armRecalibrate}
									disabled={!appState.solver_enabled}
									title={appState.solver_enabled ? '' : t('Turn on the plate solver to recalibrate')}>
									{t('Recalibrate')}
								</button>
								{#if recalNote}
									<span class="calib-note">✓ {t('Recalibration requested')}</span>
								{/if}
							{/if}
						</div>
						<p class="hint">{t('Use this after moving the camera. It recalibrates automatically within the schedule.')}</p>
					</div>
				</section>

			<!-- ── Record ────────────────────────────────── -->
			{:else if activeTab === 'record'}
				<section>
					<h2>{t('Regular Recording')}</h2>
					<label class="row">
						<span>{t('Enable')}</span>
						<input type="checkbox" checked={appState.record_enabled}
							onchange={(e) => send('set_record', e.currentTarget.checked)} />
					</label>
					<label class="row">
						<span>{t('Mode')}</span>
						<div class="select-wrap">
							<select onchange={(e) => setTwilightMode('record', e.currentTarget.value as TwilightMode)}>
								<option value="hours" selected={!appState.record_use_twilight}>{t('By hours')}</option>
								<option value="civil" selected={appState.record_use_twilight && !appState.record_twilight_astro}>{t('Civil twilight (night)')}</option>
								<option value="astro" selected={appState.record_use_twilight && appState.record_twilight_astro}>{t('Astronomical twilight (deep night)')}</option>
							</select>
						</div>
					</label>
					{#if !appState.record_use_twilight}
					<div class="row">
						<span>{t('Schedule')}</span>
						<div class="schedule-wrap">
							<select onchange={(e) => { const s = +e.currentTarget.value; appState.record_start_hour = s; appState.record_end_hour = clampEnd(s, appState.record_end_hour); sendSchedule('set_record_schedule', appState.record_start_hour, appState.record_end_hour); }}>
								{#each startHourOptions as h}
									<option value={h} selected={appState.record_start_hour === h}>{hourLabel(h)}</option>
								{/each}
							</select>
							<span class="schedule-sep">–</span>
							<select onchange={(e) => { appState.record_end_hour = +e.currentTarget.value; sendSchedule('set_record_schedule', appState.record_start_hour, appState.record_end_hour); }}>
								{#each endHourOptions(appState.record_start_hour) as h}
									<option value={h} selected={endDisplay(appState.record_start_hour, appState.record_end_hour) === h}>{hourLabel(h)}</option>
								{/each}
							</select>
						</div>
					</div>
					{/if}
				</section>

				<section>
					<h2>{t('Timelapse')}</h2>
					<label class="row">
						<span>{t('Enable')}</span>
						<input type="checkbox" checked={appState.timelapse_enabled}
							onchange={(e) => send('set_timelapse', e.currentTarget.checked)} />
					</label>
					<label class="row">
						<span>{t('Mode')}</span>
						<div class="select-wrap">
							<select onchange={(e) => setTwilightMode('timelapse', e.currentTarget.value as TwilightMode)}>
								<option value="hours" selected={!appState.timelapse_use_twilight}>{t('By hours')}</option>
								<option value="civil" selected={appState.timelapse_use_twilight && !appState.timelapse_twilight_astro}>{t('Civil twilight (night)')}</option>
								<option value="astro" selected={appState.timelapse_use_twilight && appState.timelapse_twilight_astro}>{t('Astronomical twilight (deep night)')}</option>
							</select>
						</div>
					</label>
					{#if !appState.timelapse_use_twilight}
					<div class="row">
						<span>{t('Schedule')}</span>
						<div class="schedule-wrap">
							<select onchange={(e) => { const s = +e.currentTarget.value; appState.timelapse_start_hour = s; appState.timelapse_end_hour = clampEnd(s, appState.timelapse_end_hour); sendSchedule('set_timelapse_schedule', appState.timelapse_start_hour, appState.timelapse_end_hour); }}>
								{#each startHourOptions as h}
									<option value={h} selected={appState.timelapse_start_hour === h}>{hourLabel(h)}</option>
								{/each}
							</select>
							<span class="schedule-sep">–</span>
							<select onchange={(e) => { appState.timelapse_end_hour = +e.currentTarget.value; sendSchedule('set_timelapse_schedule', appState.timelapse_start_hour, appState.timelapse_end_hour); }}>
								{#each endHourOptions(appState.timelapse_start_hour) as h}
									<option value={h} selected={endDisplay(appState.timelapse_start_hour, appState.timelapse_end_hour) === h}>{hourLabel(h)}</option>
								{/each}
							</select>
						</div>
					</div>
					{/if}
				</section>

			<!-- ── Image processing (ISP bypass) ──────────── -->
			{:else if activeTab === 'image'}
				<section>
					<h2>{t('Image Processing')}</h2>
					<p class="hint">{t('Toggle ISP modules. Settings persist across reboot.')}</p>
					{#if !bypassBits}
						<div class="empty"><span class="spinner"></span>{t('Reading current ISP state…')}</div>
					{:else}
						{#each ispToggles as tog}
							<label class="row">
								<span class="isp-label">
									{tog.key}
									{#if tog.meteorHint === 'off'}
										<span class="isp-hint isp-hint-off">{t('★ meteor: off')}</span>
									{:else if tog.meteorHint === 'on'}
										<span class="isp-hint isp-hint-on">{t('★ meteor: on')}</span>
									{/if}
								</span>
								<input type="checkbox" checked={isFeatureOn(tog.bit)}
									onchange={(e) => setFeature(tog.bit, e.currentTarget.checked)} />
							</label>
						{/each}
					{/if}
				</section>

			<!-- ── Webhook ───────────────────────────────── -->
			{:else if activeTab === 'webhook'}
				<section>
					<h2>{t('Endpoints')}</h2>
					<div class="row">
						<span>Discord</span>
						<input
							class="url-input"
							type="text"
							placeholder="https://discord.com/api/webhooks/..."
							value={appState.webhook_discord_url}
							onchange={(e) => send('set_webhook_discord', e.currentTarget.value)}
						/>
					</div>
					<div class="row">
						<span>Slack</span>
						<input
							class="url-input"
							type="text"
							placeholder="https://hooks.slack.com/services/..."
							value={appState.webhook_slack_url}
							onchange={(e) => send('set_webhook_slack', e.currentTarget.value)}
						/>
					</div>
					<div class="row">
						<span>{t('Generic')}</span>
						<input
							class="url-input"
							type="text"
							placeholder="https://..."
							value={appState.webhook_generic_url}
							onchange={(e) => send('set_webhook_generic', e.currentTarget.value)}
						/>
					</div>
				</section>
				<section>
					<h2>{t('Notifications')}</h2>
					<label class="row">
						<span>{t('Startup')}</span>
						<input type="checkbox" checked={appState.webhook_notify_startup}
							onchange={(e) => send('set_webhook_notify_startup', e.currentTarget.checked)} />
					</label>
					<label class="row">
						<span>{t('Detection Start')}</span>
						<input type="checkbox" checked={appState.webhook_notify_detection_start}
							onchange={(e) => send('set_webhook_notify_detection_start', e.currentTarget.checked)} />
					</label>
					<label class="row">
						<span>{t('Detection Stop')}</span>
						<input type="checkbox" checked={appState.webhook_notify_detection_end}
							onchange={(e) => send('set_webhook_notify_detection_end', e.currentTarget.checked)} />
					</label>
				</section>
			{/if}

		</div>
	{/if}
</div>

<style>
	.page {
		max-width: 640px;
		margin: 0 auto;
		padding: 1.25rem 1rem;
		display: flex;
		flex-direction: column;
		gap: 0;
	}

	/* ── Header ─────────────────────────────── */
	.page-header {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		margin-bottom: 1.25rem;
	}

	h1 {
		margin: 0;
		font-size: 1.25rem;
		font-weight: 700;
	}

	.ws-pill {
		display: flex;
		align-items: center;
		gap: 0.35rem;
		font-size: 0.75rem;
		color: var(--text3);
		background: var(--card);
		border-radius: 20px;
		padding: 0.2rem 0.6rem;
	}

	.ws-pill.on { color: var(--text2); }

	.ws-dot {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: #c0392b;
		transition: background 0.3s;
	}

	.ws-pill.on .ws-dot { background: #27ae60; }

	/* ── Tabs ───────────────────────────────── */
	.tabs {
		display: flex;
		gap: 0;
		border-bottom: 1px solid var(--border);
		margin-bottom: 1.5rem;
	}

	.tab {
		padding: 0.55rem 1rem;
		background: none;
		border: none;
		border-bottom: 2px solid transparent;
		margin-bottom: -1px;
		color: var(--text3);
		font-size: 0.875rem;
		font-weight: 500;
		cursor: pointer;
		transition: color 0.15s, border-color 0.15s;
		white-space: nowrap;
	}

	.tab:hover { color: var(--text2); }

	.tab.active {
		color: var(--text);
		border-bottom-color: var(--text);
	}

	/* ── Sections ───────────────────────────── */
	section {
		margin-bottom: 1.75rem;
	}

	h2 {
		margin: 0 0 0.75rem;
		font-size: 0.7rem;
		font-weight: 600;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--text3);
	}

	/* ── Setting rows ───────────────────────── */
	.row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 1rem;
		padding: 0.65rem 0;
		border-bottom: 1px solid var(--border2);
		font-size: 0.9rem;
		color: var(--text);
		cursor: pointer;
	}

	.row:last-child { border-bottom: none; }

	/* Checkbox toggle */
	.row input[type="checkbox"] {
		width: 40px;
		height: 22px;
		appearance: none;
		background: var(--toggle-off);
		border-radius: 11px;
		cursor: pointer;
		position: relative;
		flex-shrink: 0;
		transition: background 0.2s;
	}

	.row input[type="checkbox"]::after {
		content: '';
		position: absolute;
		width: 16px;
		height: 16px;
		border-radius: 50%;
		background: var(--text3);
		top: 3px;
		left: 3px;
		transition: transform 0.2s, background 0.2s;
	}

	.row input[type="checkbox"]:checked {
		background: #1a6e3c;
	}

	.row input[type="checkbox"]:checked::after {
		transform: translateX(18px);
		background: #2ecc71;
	}

	/* Slider */
	.slider-wrap {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		flex: 1;
		max-width: 240px;
	}

	.slider-val {
		font-size: 0.8rem;
		color: var(--text2);
		min-width: 2.5rem;
		text-align: right;
		font-variant-numeric: tabular-nums;
	}

	/* Select — custom arrow via ::after on wrapper */
	.select-wrap {
		position: relative;
	}

	.select-wrap::after {
		content: '';
		position: absolute;
		right: 0.65rem;
		top: 50%;
		transform: translateY(-50%);
		width: 0;
		height: 0;
		border-left: 4px solid transparent;
		border-right: 4px solid transparent;
		border-top: 5px solid var(--text3);
		pointer-events: none;
	}

	.select-wrap select {
		appearance: none;
		background: var(--input);
		border: 1px solid var(--border);
		border-radius: 6px;
		color: var(--text);
		font-size: 0.85rem;
		padding: 0.35rem 2rem 0.35rem 0.65rem;
		cursor: pointer;
		transition: border-color 0.15s;
	}

	.select-wrap select:focus {
		outline: none;
		border-color: #3498db;
	}

	.select-wrap select option {
		background: var(--input);
		color: var(--text);
	}

	/* ── URL input ──────────────────────────── */
	.url-input {
		flex: 1;
		min-width: 0;
		background: var(--input);
		border: 1px solid var(--border);
		border-radius: 6px;
		color: var(--text);
		font-size: 0.8rem;
		font-family: monospace;
		padding: 0.4rem 0.6rem;
		transition: border-color 0.15s;
	}

	.url-input:focus {
		outline: none;
		border-color: #3498db;
	}

	.url-input::placeholder {
		color: var(--text4);
	}

	.num-input {
		width: 7em;
		background: var(--input);
		border: 1px solid var(--border);
		border-radius: 6px;
		color: var(--text);
		font-size: 0.85rem;
		font-family: monospace;
		padding: 0.35rem 0.5rem;
		text-align: right;
	}
	.num-input:focus {
		outline: none;
		border-color: #3498db;
	}

	/* ── Schedule selects ───────────────────── */
	.schedule-wrap {
		display: flex;
		align-items: center;
		gap: 0.4rem;
	}

	.schedule-wrap select {
		appearance: none;
		background: var(--input);
		border: 1px solid var(--border);
		border-radius: 6px;
		color: var(--text);
		font-size: 0.85rem;
		padding: 0.35rem 0.65rem;
		cursor: pointer;
		transition: border-color 0.15s;
	}

	.schedule-wrap select:focus {
		outline: none;
		border-color: #3498db;
	}

	.schedule-sep {
		color: var(--text3);
		font-size: 0.85rem;
	}

	/* ── Empty / loading ────────────────────── */
	.empty {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		padding: 3rem 0;
		color: var(--text3);
		font-size: 0.9rem;
	}

	.spinner {
		width: 20px;
		height: 20px;
		border: 2px solid var(--toggle-off);
		border-top-color: var(--text3);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
		flex-shrink: 0;
	}

	@keyframes spin { to { transform: rotate(360deg); } }

	.isp-label {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		flex-wrap: wrap;
	}
	.isp-hint {
		font-size: 0.65rem;
		font-weight: 600;
		padding: 0.05rem 0.35rem;
		border-radius: 3px;
		letter-spacing: 0.03em;
	}
	.isp-hint-off { background: #3a1a16; color: #f88; }
	.isp-hint-on  { background: #1a3a1a; color: #8f8; }

	/* ── Astro hints ────────────────────────── */
	.hint {
		margin: 0.25rem 0 0;
		font-size: 0.75rem;
		color: var(--text3);
		line-height: 1.5;
	}

	/* ── Plate-solver calibration card ──────── */
	.calib {
		--accent: var(--text3);
		display: flex;
		flex-direction: column;
		gap: 0.85rem;
		padding: 0.9rem 1rem;
		background: var(--card);
		border: 1px solid var(--border);
		border-left: 3px solid var(--accent);
		border-radius: 8px;
	}
	.calib.ok   { --accent: #4caf78; }
	.calib.work { --accent: #6aa9ff; }
	.calib.warn { --accent: #f90; }

	.calib-head {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		font-size: 0.9rem;
		color: var(--text);
	}
	.calib-dot {
		width: 9px;
		height: 9px;
		border-radius: 50%;
		background: var(--accent);
		flex-shrink: 0;
	}
	.calib.work .calib-dot { animation: calib-pulse 1.4s ease-in-out infinite; }
	@keyframes calib-pulse {
		50% { opacity: 0.35; }
	}
	@media (prefers-reduced-motion: reduce) {
		.calib.work .calib-dot { animation: none; }
	}

	.calib-bar {
		height: 4px;
		background: var(--toggle-off);
		border-radius: 2px;
		overflow: hidden;
	}
	.calib-bar > div {
		height: 100%;
		background: var(--accent);
		transition: width 0.4s;
	}

	.calib-grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(7.5rem, 1fr));
		gap: 0.6rem 1rem;
		margin: 0;
	}
	.calib-grid > div { min-width: 0; }
	.calib-grid dt {
		font-size: 0.68rem;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--text3);
	}
	.calib-grid dd {
		margin: 0.1rem 0 0;
		font-size: 0.85rem;
		font-family: ui-monospace, 'SF Mono', Menlo, Consolas, monospace;
		color: var(--text);
	}

	.calib-actions {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 0.6rem;
	}
	.calib-warn {
		flex: 1 1 14rem;
		font-size: 0.82rem;
		color: #f90;
	}
	.calib-note {
		font-size: 0.8rem;
		color: #4caf78;
	}
	.calib .hint { margin: 0; }

	.btn {
		padding: 0.45rem 0.95rem;
		border-radius: 6px;
		border: 1px solid transparent;
		font-size: 0.85rem;
		font-weight: 500;
		cursor: pointer;
		transition: background 0.15s, border-color 0.15s, color 0.15s;
	}
	.btn:disabled { opacity: 0.4; cursor: not-allowed; }
	.btn:focus-visible { outline: 2px solid #6aa9ff; outline-offset: 2px; }
	.btn.outline {
		background: transparent;
		border-color: var(--border);
		color: var(--text);
	}
	.btn.outline:hover:not(:disabled) { background: var(--hover); border-color: var(--text3); }
	.btn.ghost {
		background: transparent;
		color: var(--text3);
	}
	.btn.ghost:hover { color: var(--text); background: var(--hover); }
	.btn.danger {
		background: #c0392b;
		color: #fff;
	}
	.btn.danger:hover { background: #d94536; }

	/* ── Mobile ─────────────────────────────── */
	@media (max-width: 480px) {
		.page { padding: 1rem 0.75rem; }

		.tab { padding: 0.5rem 0.75rem; font-size: 0.8rem; }

		.slider-wrap { max-width: none; }

		.row { font-size: 0.85rem; }
	}
</style>
