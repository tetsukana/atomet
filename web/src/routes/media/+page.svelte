<script lang="ts">
	import { onMount } from 'svelte';

	type Dir = 'records' | 'timelapse' | 'detections';
	type FileEntry = { name: string; size: number; modified: number };

	let activeDir = $state<Dir>('records');
	let files = $state<FileEntry[]>([]);
	let loading = $state(false);
	let error = $state('');

	const dirs: { id: Dir; label: string }[] = [
		{ id: 'records',    label: 'Records'    },
		{ id: 'timelapse',  label: 'Timelapse'  },
		{ id: 'detections', label: 'Detections' },
	];

	async function loadFiles(dir: Dir) {
		loading = true;
		error = '';
		try {
			const res = await fetch(`/api/files?dir=${dir}`);
			if (!res.ok) throw new Error(`HTTP ${res.status}`);
			const data = await res.json();
			files = data.files ?? [];
		} catch (e) {
			error = String(e);
			files = [];
		} finally {
			loading = false;
		}
	}

	function switchTab(dir: Dir) {
		activeDir = dir;
		loadFiles(dir);
	}

	onMount(() => loadFiles(activeDir));

	function fileUrl(name: string): string {
		return `/files/${activeDir}/${encodeURIComponent(name)}`;
	}

	function fmtSize(bytes: number): string {
		if (bytes >= 1_000_000_000) return (bytes / 1_000_000_000).toFixed(1) + ' GB';
		if (bytes >= 1_000_000)     return (bytes / 1_000_000).toFixed(1) + ' MB';
		if (bytes >= 1_000)         return (bytes / 1_000).toFixed(0) + ' KB';
		return bytes + ' B';
	}

	function fmtDate(secs: number): string {
		if (!secs) return '—';
		const d = new Date(secs * 1000);
		const yy = d.getFullYear();
		const mo = String(d.getMonth() + 1).padStart(2, '0');
		const dd = String(d.getDate()).padStart(2, '0');
		const hh = String(d.getHours()).padStart(2, '0');
		const mm = String(d.getMinutes()).padStart(2, '0');
		return `${yy}-${mo}-${dd} ${hh}:${mm}`;
	}

	function isVideo(name: string): boolean {
		return /\.(mp4|mkv|avi|mov)$/i.test(name);
	}

	function isImage(name: string): boolean {
		return /\.(png|jpg|jpeg)$/i.test(name);
	}
</script>

<div class="page">
	<div class="page-header">
		<h1>Media</h1>
		<span class="file-count">{files.length} files</span>
	</div>

	<!-- Tabs -->
	<div class="tabs" role="tablist">
		{#each dirs as dir}
			<button
				role="tab"
				class="tab"
				class:active={activeDir === dir.id}
				aria-selected={activeDir === dir.id}
				onclick={() => switchTab(dir.id)}
			>
				{dir.label}
			</button>
		{/each}
		<button class="refresh" onclick={() => loadFiles(activeDir)} disabled={loading} title="Refresh">
			<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
				stroke-linecap="round" stroke-linejoin="round"
				class:spinning={loading}>
				<polyline points="23 4 23 10 17 10"/>
				<path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/>
			</svg>
		</button>
	</div>

	<!-- Content -->
	{#if loading}
		<div class="empty">
			<span class="spinner"></span>
			Loading…
		</div>
	{:else if error}
		<div class="empty err">{error}</div>
	{:else if files.length === 0}
		<div class="empty">No files in /media/mmc/{activeDir}</div>
	{:else if activeDir === 'detections'}
		<div class="thumb-grid">
			{#each files as f}
				<a class="thumb-card" href={fileUrl(f.name)} target="_blank" rel="noopener noreferrer">
					<img src={fileUrl(f.name)} alt={f.name} loading="lazy" />
					<span class="thumb-name">{f.name.replace(/\.png$/i, '')}</span>
				</a>
			{/each}
		</div>
	{:else}
		<div class="file-list">
			{#each files as f}
				<a
					class="file-row"
					href={fileUrl(f.name)}
					target="_blank"
					rel="noopener noreferrer"
					download={!isVideo(f.name) ? f.name : undefined}
				>
					<span class="file-icon">
						{#if isVideo(f.name)}
							<svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
								<rect x="2" y="5" width="15" height="11" rx="2" fill="#3498db"/>
								<path d="M17 9l5-3.5v9L17 11V9z" fill="#3498db"/>
							</svg>
						{:else}
							<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
								<path d="M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"/>
								<polyline points="13 2 13 9 20 9"/>
							</svg>
						{/if}
					</span>
					<span class="file-name">{f.name}</span>
					<span class="file-date">{fmtDate(f.modified)}</span>
					<span class="file-size">{fmtSize(f.size)}</span>
					<span class="file-action">
						{#if isVideo(f.name)}
							<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
								<circle cx="12" cy="12" r="10"/>
								<polygon points="10 8 16 12 10 16 10 8"/>
							</svg>
						{:else}
							<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
								<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/>
								<polyline points="7 10 12 15 17 10"/>
								<line x1="12" y1="15" x2="12" y2="3"/>
							</svg>
						{/if}
					</span>
				</a>
			{/each}
		</div>
	{/if}
</div>

<style>
	.page {
		max-width: 800px;
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

	.file-count {
		font-size: 0.8rem;
		color: var(--text3);
		background: var(--card);
		border-radius: 20px;
		padding: 0.15rem 0.55rem;
	}

	/* ── Tabs ───────────────────────────────── */
	.tabs {
		display: flex;
		align-items: center;
		gap: 0;
		border-bottom: 1px solid var(--border);
		margin-bottom: 0;
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
	}

	.tab:hover  { color: var(--text2); }
	.tab.active { color: var(--text); border-bottom-color: var(--text); }

	.refresh {
		margin-left: auto;
		background: none;
		border: none;
		color: var(--text3);
		cursor: pointer;
		padding: 0.4rem;
		border-radius: 6px;
		display: flex;
		align-items: center;
		transition: color 0.15s, background 0.15s;
	}

	.refresh:hover    { color: var(--text2); background: var(--hover); }
	.refresh:disabled { opacity: 0.4; cursor: default; }

	.refresh svg.spinning {
		animation: spin 0.7s linear infinite;
	}

	@keyframes spin { to { transform: rotate(360deg); } }

	/* ── File list ──────────────────────────── */
	.file-list {
		border: 1px solid var(--border);
		border-top: none;
		border-radius: 0 0 8px 8px;
		overflow: hidden;
	}

	.file-row {
		display: grid;
		grid-template-columns: 20px 1fr auto auto 20px;
		align-items: center;
		gap: 0.75rem;
		padding: 0.6rem 0.75rem;
		border-top: 1px solid var(--border2);
		color: var(--text);
		text-decoration: none;
		font-size: 0.85rem;
		transition: background 0.12s;
	}

	.file-row:first-child { border-top: none; }
	.file-row:hover { background: var(--hover); }

	.file-icon { display: flex; align-items: center; color: var(--text3); flex-shrink: 0; }
	.file-row:hover .file-icon { color: var(--text2); }

	.file-name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-family: monospace;
		font-size: 0.8rem;
	}

	.file-date {
		color: var(--text3);
		font-size: 0.75rem;
		white-space: nowrap;
		flex-shrink: 0;
		font-variant-numeric: tabular-nums;
	}

	.file-size {
		color: var(--text3);
		font-size: 0.75rem;
		white-space: nowrap;
		flex-shrink: 0;
		text-align: right;
		font-variant-numeric: tabular-nums;
		min-width: 4rem;
	}

	.file-action { display: flex; align-items: center; color: var(--text4); flex-shrink: 0; }
	.file-row:hover .file-action { color: var(--text2); }

	/* ── Detection thumbnails ───────────────── */
	.thumb-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
		gap: 0.75rem;
		padding: 0.75rem 0;
	}

	.thumb-card {
		display: flex;
		flex-direction: column;
		gap: 0.3rem;
		text-decoration: none;
		color: var(--text);
		border: 1px solid var(--border);
		border-radius: 6px;
		overflow: hidden;
		transition: border-color 0.15s;
	}

	.thumb-card:hover { border-color: var(--text3); }

	.thumb-card img {
		width: 100%;
		aspect-ratio: 16/9;
		object-fit: cover;
		background: #111;
		display: block;
	}

	.thumb-name {
		padding: 0.25rem 0.4rem;
		font-size: 0.7rem;
		font-family: monospace;
		color: var(--text3);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	/* ── Empty / error ──────────────────────── */
	.empty {
		padding: 3rem 0.75rem;
		color: var(--text3);
		font-size: 0.875rem;
		display: flex;
		align-items: center;
		gap: 0.75rem;
		border: 1px solid var(--border);
		border-top: none;
		border-radius: 0 0 8px 8px;
	}

	.empty.err { color: #c0392b; }

	.spinner {
		width: 18px;
		height: 18px;
		border: 2px solid var(--toggle-off);
		border-top-color: var(--text3);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
		flex-shrink: 0;
	}

	/* ── Mobile ─────────────────────────────── */
	@media (max-width: 480px) {
		.page { padding: 1rem 0.75rem; }

		.file-row {
			grid-template-columns: 20px 1fr auto 20px;
			gap: 0.5rem;
		}

		/* hide date on small screens */
		.file-date { display: none; }

		.file-size { min-width: 3rem; }
	}
</style>
