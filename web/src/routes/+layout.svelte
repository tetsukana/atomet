<script lang="ts">
	import { page } from '$app/state';
	import { onMount } from 'svelte';

	const { children } = $props();

	let theme = $state<'dark' | 'light'>('dark');

	onMount(() => {
		const saved = localStorage.getItem('atomet-theme');
		if (saved === 'light' || saved === 'dark') theme = saved;
		document.documentElement.dataset.theme = theme;
	});

	function toggleTheme() {
		theme = theme === 'dark' ? 'light' : 'dark';
		document.documentElement.dataset.theme = theme;
		localStorage.setItem('atomet-theme', theme);
	}
</script>

<div class="app">
	<header class="topbar">
		<a class="brand" href="/" aria-label="atomet">			
		<svg width="140" height="18" viewBox="0 0 76.411606 10.343434" version="1.1"
   			id="svg1"
   			xmlns="http://www.w3.org/2000/svg"
   			xmlns:svg="http://www.w3.org/2000/svg">
  			<defs id="defs1" />
  			<g id="layer1" transform="translate(-62.810903,-96.540685)">
    			<g id="text16" style="font-size:14.1111px;font-family:Neuropol;-inkscape-font-specification:'Neuropol, Normal';fill:#2164c1;stroke:#1abc9c;stroke-width:0;stroke-miterlimit:0" aria-label="ATOMET">
      				<path style="fill:var(--text)"
        				d="m 69.81001,96.540687 c -0.564444,0 -0.90311,0.183444 -1.114777,0.493889 l -5.785551,8.777104 c -0.05644,0.0988 -0.09878,0.19756 -0.09878,0.31044 0,0.43745 0.550333,0.762 1.015999,0.762 0.239889,0 0.479778,-0.0847 0.606778,-0.28222 l 1.566332,-2.42711 h 7.845772 l 1.665109,2.42711 c 0.141111,0.19756 0.366889,0.28222 0.620889,0.28222 0.479777,0 1.03011,-0.32455 1.03011,-0.74788 0,-0.0988 -0.02822,-0.21167 -0.09878,-0.32456 L 70.910676,97.034576 C 70.684898,96.724131 70.360343,96.540687 69.81001,96.540687 Z m 0,1.721554 2.991553,4.388549 h -5.827884 z m 15.663339,-0.155222 c 0.451555,0 0.550333,-0.254 0.550333,-0.761999 0,-0.508 -0.09878,-0.762 -0.550333,-0.762 h -10.38577 c -0.451555,0 -0.550333,0.254 -0.550333,0.762 0,0.507999 0.09878,0.761999 0.550333,0.761999 h 4.247442 v 8.099771 c 0,0.53622 0.324555,0.66322 0.931332,0.66322 0.620889,0 0.945444,-0.127 0.945444,-0.66322 v -8.099771 z m 9.256867,8.706551 c 2.779886,0 4.176885,-0.87489 4.176885,-2.62467 v -4.981215 c 0,-1.763887 -1.396999,-2.624665 -4.176885,-2.624665 h -4.10633 c -2.779887,0 -4.176886,0.860778 -4.176886,2.624665 v 4.981215 c 0,1.74978 1.396999,2.62467 4.176886,2.62467 z m -0.05644,-1.524 H 90.68033 c -1.650999,0 -2.370665,-0.49389 -2.370665,-1.10067 v -4.981215 c 0,-0.620888 0.719666,-1.100666 2.370665,-1.100666 h 3.993441 c 1.650999,0 2.370665,0.479778 2.370665,1.100666 v 4.981215 c 0,0.60678 -0.719666,1.10067 -2.370665,1.10067 z"
        				id="path19" />
      				<path
						d="m 108.74253,104.93679 -6.4911,-7.831659 c -0.28223,-0.324555 -0.635,-0.493888 -1.11478,-0.493888 -0.67733,0 -1.03011,0.324555 -1.03011,0.959554 v 8.635993 c 0,0.53622 0.32455,0.66322 0.93133,0.66322 0.62089,0 0.94545,-0.127 0.94545,-0.66322 v -6.688661 l 5.64444,6.815661 c 0.29633,0.36689 0.64911,0.53622 1.18533,0.53622 0.49389,0 0.79022,-0.15522 1.11477,-0.53622 l 5.63033,-6.815661 v 6.688661 c 0,0.53622 0.32456,0.66322 0.94545,0.66322 0.62089,0 0.94544,-0.127 0.94544,-0.66322 v -8.635993 c 0,-0.634999 -0.35278,-0.959554 -1.04422,-0.959554 -0.45156,0 -0.81844,0.141111 -1.11478,0.493888 z"
        				id="path20" />
      				<path style="fill:var(--text)"
        				d="m 129.6693,102.5379 c 0.45155,0 0.55033,-0.254 0.55033,-0.76199 0,-0.508 -0.0988,-0.762 -0.55033,-0.762 h -9.15811 v -1.806225 c 0,-0.620888 0.71967,-1.100666 2.37067,-1.100666 h 2.04611 c 0.43744,0 0.53622,-0.254 0.53622,-0.761999 0,-0.508 -0.0988,-0.762 -0.53622,-0.762 h -2.10256 c -2.77988,0 -4.17688,0.860778 -4.17688,2.624665 v 4.981215 c 0,1.74978 1.397,2.62467 4.17688,2.62467 h 6.80155 c 0.45156,0 0.55034,-0.26811 0.55034,-0.762 0,-0.508 -0.0988,-0.762 -0.55034,-0.762 h -6.7451 c -1.651,0 -2.37067,-0.49389 -2.37067,-1.10067 v -1.651 z m 9.00288,-4.430881 c 0.45155,0 0.55033,-0.254 0.55033,-0.761999 0,-0.508 -0.0988,-0.762 -0.55033,-0.762 h -10.38577 c -0.45156,0 -0.55033,0.254 -0.55033,0.762 0,0.507999 0.0988,0.761999 0.55033,0.761999 h 4.24744 v 8.099771 c 0,0.53622 0.32455,0.66322 0.93133,0.66322 0.62089,0 0.94545,-0.127 0.94545,-0.66322 v -8.099771 z"
        				id="path21" />
    				</g>
  				</g>
			</svg>
		</a>

		<nav class="nav">
			<a href="/" class:active={page.url.pathname === '/'}>
				<svg width="15" height="15" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
					<rect x="2" y="5" width="15" height="11" rx="2"/>
					<path d="M17 9l5-3.5v9L17 11V9z"/>
				</svg>
				Live
			</a>
			<a href="/media" class:active={page.url.pathname.startsWith('/media')}>
				<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
					<path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>
				</svg>
				Media
			</a>
			<a href="/mask" class:active={page.url.pathname.startsWith('/mask')}>
				<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
					<rect x="3" y="3" width="18" height="18" rx="2"/>
					<path d="M3 9h18M3 15h18M9 3v18M15 3v18" opacity="0.4"/>
				</svg>
				Mask
			</a>
			<a href="/settings" class:active={page.url.pathname.startsWith('/settings')}>
				<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
					<circle cx="12" cy="12" r="3"/>
					<path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>
				</svg>
				Settings
			</a>
		</nav>

		<button class="theme-btn" onclick={toggleTheme}
			title={theme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
			aria-label={theme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}>
			{#if theme === 'dark'}
				<!-- Sun -->
				<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<circle cx="12" cy="12" r="5"/>
					<line x1="12" y1="1"     x2="12" y2="3"/>
					<line x1="12" y1="21"    x2="12" y2="23"/>
					<line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/>
					<line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/>
					<line x1="1"  y1="12" x2="3"  y2="12"/>
					<line x1="21" y1="12" x2="23" y2="12"/>
					<line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/>
					<line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/>
				</svg>
			{:else}
				<!-- Moon -->
				<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/>
				</svg>
			{/if}
		</button>
	</header>

	<main class="main">
		{@render children()}
	</main>
</div>

<style>
	/* ── Theme variables ────────────────────────── */
	:global(:root) {
		--bg:         #0d0d0d;
		--card:       #111111;
		--input:      #1e1e1e;
		--hover:      #1a1a1a;
		--border:     #1e1e1e;
		--border2:    #181818;
		--text:       #e0e0e0;
		--text2:      #888888;
		--text3:      #555555;
		--text4:      #333333;
		--toggle-off: #2a2a2a;
		--chart-grid: #1e1e1e;
	}

	:global([data-theme="light"]) {
		--bg:         #ececef;
		--card:       #ffffff;
		--input:      #f3f3f6;
		--hover:      #f0f0f3;
		--border:     #dddde3;
		--border2:    #eeeeef;
		--text:       #111827;
		--text2:      #6b7280;
		--text3:      #9ca3af;
		--text4:      #d1d5db;
		--toggle-off: #d1d5db;
		--chart-grid: #e0e0e4;
	}

	/* ── Global resets ──────────────────────────── */
	:global(*, *::before, *::after) { box-sizing: border-box; }

	:global(body) {
		margin: 0;
		font-family: system-ui, -apple-system, sans-serif;
		background: var(--bg);
		color: var(--text);
		transition: background 0.2s, color 0.2s;
	}

	:global(a) { color: inherit; text-decoration: none; }

	.app {
		min-height: 100vh;
		display: flex;
		flex-direction: column;
	}

	/* ── Topbar ─────────────────────────────────── */
	.topbar {
		position: sticky;
		top: 0;
		z-index: 100;
		display: flex;
		align-items: center;
		height: 48px;
		padding: 0 0.75rem;
		background: var(--card);
		border-bottom: 1px solid var(--border);
		transition: background 0.2s, border-color 0.2s;
	}

	.brand {
		display: flex;
		align-items: center;
		gap: 0.45rem;
		font-weight: 700;
		font-size: 1rem;
		letter-spacing: 0.04em;
		color: var(--text);
		margin-right: auto;
	}

	/* ── Nav links ──────────────────────────────── */
	.nav {
		display: flex;
		gap: 0.15rem;
	}

	.nav a {
		display: flex;
		align-items: center;
		gap: 0.35rem;
		padding: 0.35rem 0.65rem;
		border-radius: 6px;
		font-size: 0.875rem;
		color: var(--text3);
		transition: color 0.15s, background 0.15s;
		white-space: nowrap;
	}

	.nav a:hover  { color: var(--text2); background: var(--hover); }
	.nav a.active { color: var(--text);  background: var(--input); }

	/* ── Theme toggle ───────────────────────────── */
	.theme-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		background: none;
		border: none;
		color: var(--text3);
		cursor: pointer;
		padding: 0.4rem;
		border-radius: 6px;
		margin-left: 0.25rem;
		transition: color 0.15s, background 0.15s;
	}

	.theme-btn:hover { color: var(--text2); background: var(--hover); }

	/* Hide nav text on very narrow screens */
	@media (max-width: 360px) {
		.nav a { padding: 0.4rem 0.45rem; gap: 0; font-size: 0; }
	}

	.main { flex: 1; }
</style>
