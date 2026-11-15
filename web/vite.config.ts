import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// 開発時の接続先デバイス。例: ATOMET_HOST=192.168.1.50 npm run dev
const host = process.env.ATOMET_HOST || 'atomet.local';

export default defineConfig({
	plugins: [sveltekit()],
	server: {
		proxy: {
			'/ws': {
				target: `ws://${host}`,
				ws: true
			},
			'/api': {
				target: `http://${host}`
			},
			// Proxy go2rtc WS/HTTP API for local dev (device exposes :1984 directly)
			'/go2rtc': {
				target: `http://${host}:1984`,
				ws: true,
				rewrite: (path) => path.replace(/^\/go2rtc/, '')
			},
			// Proxy recorded/timelapse file downloads
			'/files': {
				target: `http://${host}`
			}
		}
	}
});
