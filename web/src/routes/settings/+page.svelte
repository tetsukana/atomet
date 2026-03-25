<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { AtometWebSocket } from '$lib/ws';
	import type { AppState } from '$lib/types';

	let ws: AtometWebSocket;
	let appState = $state<AppState | null>(null);
	let wsConnected = $state(false);

	type Tab = 'system' | 'detection' | 'record';
	let activeTab = $state<Tab>('system');

	const tabs: { id: Tab; label: string }[] = [
		{ id: 'system',    label: 'System'    },
		{ id: 'detection', label: 'Detection' },
		{ id: 'record',    label: 'Record'    },
	];

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
		ws.connect();
	});

	onDestroy(() => ws?.disconnect());

	function send(type: string, value: unknown) {
		ws?.send({ type, value });
	}

	function sendSchedule(type: string, start: number, end: number) {
		ws?.send({ type, start, end });
	}

	function hourLabel(h: number): string {
		if (h < 24) return `${h}:00`;
		return `${h - 24}:00 (+1)`;
	}

	const hourOptions = Array.from({ length: 31 }, (_, i) => i);
</script>

<div class="page">
	<!-- Page header -->
	<div class="page-header">
		<h1>Settings</h1>
		<span class="ws-pill" class:on={wsConnected}>
			<span class="ws-dot"></span>
			{wsConnected ? 'Connected' : 'Disconnected'}
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
			Connecting…
		</div>
	{:else}
		<div class="content" role="tabpanel">

			<!-- ── System ─────────────────────────────────── -->
			{#if activeTab === 'system'}
				<section>
					<h2>Camera</h2>
					<label class="row">
						<span>Auto Day/Night</span>
						<input type="checkbox" checked={appState.auto_daynight}
							onchange={(e) => send('set_auto_daynight', e.currentTarget.checked)} />
					</label>
					<label class="row">
						<span>Night Mode</span>
						<input type="checkbox" checked={appState.night_mode}
							onchange={(e) => send('set_night_mode', e.currentTarget.checked)} />
					</label>
					<label class="row">
						<span>IR Cut Filter</span>
						<input type="checkbox" checked={appState.ircut_on}
							onchange={(e) => send('set_ircut', e.currentTarget.checked)} />
					</label>
					<label class="row">
						<span>LED</span>
						<input type="checkbox" checked={appState.led_on}
							onchange={(e) => send('set_led', e.currentTarget.checked)} />
					</label>
					<label class="row">
						<span>IR LED</span>
						<input type="checkbox" checked={appState.irled_on}
							onchange={(e) => send('set_irled', e.currentTarget.checked)} />
					</label>
				</section>

				<section>
					<h2>OSD</h2>
					<label class="row">
						<span>Show Timestamp</span>
						<input type="checkbox" checked={appState.show_timestamp}
							onchange={(e) => send('set_show_timestamp', e.currentTarget.checked)} />
					</label>
				<label class="row">
					<span>Timestamp Position</span>
					<div class="select-wrap">
						<select
							onchange={(e) => send('set_timestamp_position', +e.currentTarget.value)}
						>
							<option value={0} selected={appState.timestamp_position === 0}>Top Left</option>
							<option value={1} selected={appState.timestamp_position === 1}>Top Right</option>
							<option value={2} selected={appState.timestamp_position === 2}>Bottom Left</option>
							<option value={3} selected={appState.timestamp_position === 3}>Bottom Right</option>
						</select>
					</div>
				</label>
				</section>

			<!-- ── Detection ─────────────────────────────── -->
			{:else if activeTab === 'detection'}
				<section>
					<h2>Detection</h2>
					<label class="row">
						<span>Enable Detection</span>
						<input type="checkbox" checked={appState.detection_enabled}
							onchange={(e) => send('set_detection', e.currentTarget.checked)} />
					</label>
				</section>

			<!-- ── Record ────────────────────────────────── -->
			{:else if activeTab === 'record'}
				<section>
					<h2>Regular Recording</h2>
					<label class="row">
						<span>Enable</span>
						<input type="checkbox" checked={appState.record_enabled}
							onchange={(e) => send('set_record', e.currentTarget.checked)} />
					</label>
					<div class="row">
						<span>Schedule</span>
						<div class="schedule-wrap">
							<select onchange={(e) => sendSchedule('set_record_schedule', +e.currentTarget.value, appState.record_end_hour)}>
								{#each hourOptions as h}
									<option value={h} selected={appState.record_start_hour === h}>{hourLabel(h)}</option>
								{/each}
							</select>
							<span class="schedule-sep">–</span>
							<select onchange={(e) => sendSchedule('set_record_schedule', appState.record_start_hour, +e.currentTarget.value)}>
								{#each hourOptions as h}
									<option value={h} selected={appState.record_end_hour === h}>{hourLabel(h)}</option>
								{/each}
							</select>
						</div>
					</div>
				</section>

				<section>
					<h2>Timelapse</h2>
					<label class="row">
						<span>Enable</span>
						<input type="checkbox" checked={appState.timelapse_enabled}
							onchange={(e) => send('set_timelapse', e.currentTarget.checked)} />
					</label>
					<div class="row">
						<span>Schedule</span>
						<div class="schedule-wrap">
							<select onchange={(e) => sendSchedule('set_timelapse_schedule', +e.currentTarget.value, appState.timelapse_end_hour)}>
								{#each hourOptions as h}
									<option value={h} selected={appState.timelapse_start_hour === h}>{hourLabel(h)}</option>
								{/each}
							</select>
							<span class="schedule-sep">–</span>
							<select onchange={(e) => sendSchedule('set_timelapse_schedule', appState.timelapse_start_hour, +e.currentTarget.value)}>
								{#each hourOptions as h}
									<option value={h} selected={appState.timelapse_end_hour === h}>{hourLabel(h)}</option>
								{/each}
							</select>
						</div>
					</div>
				</section>

				<section>
					<h2>Detection Recording</h2>
					<label class="row">
						<span>Enable</span>
						<input type="checkbox" checked={appState.detection_record_enabled}
							onchange={(e) => send('set_detection_record', e.currentTarget.checked)} />
					</label>
					<div class="row">
						<span>Schedule</span>
						<div class="schedule-wrap">
							<select onchange={(e) => sendSchedule('set_detection_record_schedule', +e.currentTarget.value, appState.detection_record_end_hour)}>
								{#each hourOptions as h}
									<option value={h} selected={appState.detection_record_start_hour === h}>{hourLabel(h)}</option>
								{/each}
							</select>
							<span class="schedule-sep">–</span>
							<select onchange={(e) => sendSchedule('set_detection_record_schedule', appState.detection_record_start_hour, +e.currentTarget.value)}>
								{#each hourOptions as h}
									<option value={h} selected={appState.detection_record_end_hour === h}>{hourLabel(h)}</option>
								{/each}
							</select>
						</div>
					</div>
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

	.slider-wrap input[type="range"] {
		flex: 1;
		height: 4px;
		appearance: none;
		background: var(--toggle-off);
		border-radius: 2px;
		cursor: pointer;
		accent-color: #3498db;
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

	/* ── Astro hints ────────────────────────── */
	.hint {
		margin: 0.25rem 0 0;
		font-size: 0.75rem;
		color: var(--text3);
		line-height: 1.5;
	}

	section.disabled {
		opacity: 0.4;
		pointer-events: none;
	}

	/* ── Mobile ─────────────────────────────── */
	@media (max-width: 480px) {
		.page { padding: 1rem 0.75rem; }

		.tab { padding: 0.5rem 0.75rem; font-size: 0.8rem; }

		.slider-wrap { max-width: none; }

		.row { font-size: 0.85rem; }
	}
</style>
