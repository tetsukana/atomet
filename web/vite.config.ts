import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [sveltekit()],
	server: {
		proxy: {
			'/ws': {
				target: 'ws://atomet.local',
				ws: true
			},
			'/api': {
				target: 'http://atomet.local'
			},
			// Proxy go2rtc WS/HTTP API for local dev (device exposes :1984 directly)
			'/go2rtc': {
				target: 'http://atomet.local:1984',
				ws: true,
				rewrite: (path) => path.replace(/^\/go2rtc/, '')
			},
			// Proxy recorded/timelapse file downloads
			'/files': {
				target: 'http://atomet.local'
			}
		}
	}
});
