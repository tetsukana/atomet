/**
 * go2rtc player — follows the protocol from go2rtc/www/video-rtc.js
 *
 * Single WebSocket, concurrent MSE + WebRTC, MJPEG fallback via WS binary.
 * WS endpoint: ws://<host>:1984/api/ws?src=<stream>  (go2rtc directly)
 */

export type PlayerMode   = 'webrtc' | 'mse' | 'hls' | 'mjpeg';
export type PlayerStatus = 'idle' | 'connecting' | 'playing' | 'error';

const CODECS = [
	'avc1.640029',       // H.264 high 4.1
	'avc1.64002A',       // H.264 high 4.2
	'avc1.640033',       // H.264 high 5.1
	'hvc1.1.6.L153.B0', // H.265 main 5.1
	'mp4a.40.2',         // AAC LC
	'mp4a.40.5',         // AAC HE
	'flac',
	'opus',
];

type MsgHandler = (msg: { type: string; value: string }) => void;

export class Go2rtcPlayer {
	private readonly streamName: string;
	private videoEl: HTMLVideoElement | null = null;

	private ws: WebSocket | null = null;
	private pc: RTCPeerConnection | null = null;

	private onmessage: Record<string, MsgHandler> = {};
	private ondata: ((data: ArrayBuffer) => void) | null = null;

	private stopped      = false;
	private pcActive     = false; // true while WebRTC is the active transport
	private connectTS    = 0;
	private reconnectTid: ReturnType<typeof setTimeout> | null = null;

	private _mode:   PlayerMode | null = null;
	private _status: PlayerStatus = 'idle';

	onModeChange:   ((mode: PlayerMode | null) => void) | null = null;
	onStatusChange: ((status: PlayerStatus) => void) | null = null;
	onBandwidth:    ((bytesPerSec: number) => void) | null = null;

	private _rxBytes = 0;
	private _bwTimer: ReturnType<typeof setInterval> | null = null;
	private _lastRtcBytes = 0;

	constructor(streamName: string) {
		this.streamName = streamName;
	}

	get mode()   { return this._mode; }
	get status() { return this._status; }

	mount(videoEl: HTMLVideoElement): void {
		this.videoEl = videoEl;
		this.stopped  = false;
		this.startBwTimer();
		this.connect();
	}

	unmount(): void {
		this.stopped = true;
		this.stopBwTimer();
		this.clearReconnect();
		this.close();
		this.setMode(null);
		this.setStatus('idle');
	}

	// ── private helpers ────────────────────────────────────────────────────────

	private setMode(m: PlayerMode | null) { this._mode = m; this.onModeChange?.(m); }
	private setStatus(s: PlayerStatus)    { this._status = s; this.onStatusChange?.(s); }

	private wsUrl(): string {
		const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
		const port = location.port;
		// On Vite dev server (non-standard port), use the /go2rtc proxy path.
		// On device (port 80/443), connect directly to go2rtc on port 1984.
		if (port && port !== '80' && port !== '443') {
			return `${proto}//${location.host}/go2rtc/api/ws?src=${this.streamName}`;
		}
		return `${proto}//${location.hostname}:1984/api/ws?src=${this.streamName}`;
	}

	private supportedCodecs(fn: (type: string) => boolean): string {
		return CODECS.filter((c) => fn(`video/mp4; codecs="${c}"`)).join();
	}

	// ── connection lifecycle ───────────────────────────────────────────────────

	private connect(): void {
		if (this.stopped || this.ws || this.pcActive) return;

		this.setStatus('connecting');
		this.connectTS = Date.now();

		const ws = new WebSocket(this.wsUrl());
		ws.binaryType = 'arraybuffer';
		this.ws = ws;

		ws.addEventListener('open', () => this.onopen());
		ws.addEventListener('close', () => this.onclose());
		ws.addEventListener('message', (ev) => {
			if (typeof ev.data === 'string') {
				this._rxBytes += ev.data.length;
				try {
					const msg = JSON.parse(ev.data) as { type: string; value: string };
					for (const h of Object.values(this.onmessage)) h(msg);
				} catch { /* ignore parse errors */ }
			} else {
				this._rxBytes += (ev.data as ArrayBuffer).byteLength;
				this.ondata?.(ev.data as ArrayBuffer);
			}
		});
	}

	private close(): void {
		if (this.ws) { this.ws.close(); this.ws = null; }
		if (this.pc) { this.pc.close(); this.pc = null; }
		const v = this.videoEl;
		if (v) {
			if (v.srcObject) {
				(v.srcObject as MediaStream).getTracks().forEach((t) => t.stop());
				v.srcObject = null;
			}
			v.src = '';
			v.load();
		}
	}

	private onopen(): void {
		this.onmessage = {};
		this.ondata    = null;

		const hasMSE    = 'MediaSource' in window || 'ManagedMediaSource' in window;
		const hasWebRTC = 'RTCPeerConnection' in window;
		const modes: string[] = [];

		if (hasMSE)    { modes.push('mse');    this.startMSE(); }
		if (hasWebRTC) { modes.push('webrtc'); this.startWebRTC(); }

		if (modes.length === 0) {
			this.startMJPEG();
		} else {
			// Fallback: if the first mode returns an error, switch to MJPEG
			this.onmessage['_mjpeg_fb'] = (msg) => {
				if (msg.type !== 'error' || !msg.value.startsWith(modes[0])) return;
				delete this.onmessage['_mjpeg_fb'];
				this.startMJPEG();
			};
		}
	}

	private onclose(): void {
		this.ws = null;
		if (this.stopped || this.pcActive) return;

		this.setMode(null);
		this.setStatus('error');

		const delay = Math.max(3000 - (Date.now() - this.connectTS), 0);
		this.reconnectTid = setTimeout(() => {
			this.reconnectTid = null;
			this.connect();
		}, delay);
	}

	private clearReconnect(): void {
		if (this.reconnectTid) { clearTimeout(this.reconnectTid); this.reconnectTid = null; }
	}

	private startBwTimer(): void {
		this.stopBwTimer();
		this._bwTimer = setInterval(() => {
			// For WebRTC, poll getStats for bytesReceived
			if (this.pcActive && this.pc) {
				this.pc.getStats().then((stats) => {
					let total = 0;
					stats.forEach((report) => {
						if (report.type === 'inbound-rtp') {
							total += (report as any).bytesReceived || 0;
						}
					});
					const delta = total - this._lastRtcBytes;
					this._lastRtcBytes = total;
					this.onBandwidth?.(delta);
				}).catch(() => {});
			} else {
				// MSE/MJPEG: report WS byte counter
				this.onBandwidth?.(this._rxBytes);
				this._rxBytes = 0;
			}
		}, 1000);
	}

	private stopBwTimer(): void {
		if (this._bwTimer) { clearInterval(this._bwTimer); this._bwTimer = null; }
	}

	// ── MSE ───────────────────────────────────────────────────────────────────

	private startMSE(): void {
		const useMMS = 'ManagedMediaSource' in window;
		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		const MS = useMMS ? (window as any).ManagedMediaSource : MediaSource;
		const ms: MediaSource = new MS();

		if (useMMS) {
			// eslint-disable-next-line @typescript-eslint/no-explicit-any
			(this.videoEl as any).disableRemotePlayback = true;
			this.videoEl!.srcObject = ms as unknown as MediaStream;
		} else {
			const url = URL.createObjectURL(ms);
			this.videoEl!.srcObject = null;
			this.videoEl!.src = url;
		}

		this.videoEl!.play().catch(() => {
			this.videoEl!.muted = true;
			this.videoEl!.play().catch(() => {});
		});

		ms.addEventListener('sourceopen', () => {
			if (!useMMS) URL.revokeObjectURL(this.videoEl!.src);
			const codecs = this.supportedCodecs(MS.isTypeSupported.bind(MS));
			this.ws?.send(JSON.stringify({ type: 'mse', value: codecs }));
		}, { once: true });

		this.onmessage['mse'] = (msg) => {
			if (msg.type !== 'mse') return;

			const sb = ms.addSourceBuffer(msg.value);
			sb.mode = 'segments';

			// Ring buffer for backpressure (2 MB)
			const buf = new Uint8Array(2 * 1024 * 1024);
			let bufLen = 0;

			sb.addEventListener('updateend', () => {
				if (!sb.updating && bufLen > 0) {
					try { sb.appendBuffer(buf.slice(0, bufLen)); bufLen = 0; } catch { /* ignore */ }
				}
				// Keep latency low: trim buffer and adjust playback rate
				if (!sb.updating && sb.buffered.length) {
					const end    = sb.buffered.end(sb.buffered.length - 1);
					const start  = end - 5;
					const start0 = sb.buffered.start(0);
					if (start > start0) {
						try { sb.remove(start0, start); } catch { /* ignore */ }
						try { ms.setLiveSeekableRange(start, end); } catch { /* ignore */ }
					}
					if (this.videoEl!.currentTime < start) this.videoEl!.currentTime = start;
					const gap = end - this.videoEl!.currentTime;
					this.videoEl!.playbackRate = gap > 0.1 ? gap : 0.1;
				}
			});

			this.ondata = (data: ArrayBuffer) => {
				if (sb.updating || bufLen > 0) {
					const b = new Uint8Array(data);
					buf.set(b, bufLen);
					bufLen += b.byteLength;
				} else {
					try { sb.appendBuffer(data); } catch { /* ignore */ }
				}
			};

			this.setMode('mse');
			this.setStatus('playing');
		};
	}

	// ── WebRTC ────────────────────────────────────────────────────────────────

	private startWebRTC(): void {
		const pc = new RTCPeerConnection({ bundlePolicy: 'max-bundle', iceServers: [] });
		this.pc = pc;

		pc.addEventListener('icecandidate', (ev) => {
			const candidate = ev.candidate?.toJSON().candidate ?? '';
			this.ws?.send(JSON.stringify({ type: 'webrtc/candidate', value: candidate }));
		});

		pc.addEventListener('connectionstatechange', () => {
			if (pc.connectionState === 'connected') {
				const tracks = pc.getTransceivers()
					.filter((t) => t.currentDirection === 'recvonly')
					.map((t) => t.receiver.track);

				const v2 = document.createElement('video');
				v2.srcObject = new MediaStream(tracks);
				v2.addEventListener('loadeddata', () => {
					if (this.pc !== pc) return; // stale
					this.videoEl!.srcObject = v2.srcObject;
					this.videoEl!.play().catch(() => {
						this.videoEl!.muted = true;
						this.videoEl!.play().catch(() => {});
					});
					v2.srcObject = null;

					this.pcActive = true;
					this.setMode('webrtc');
					this.setStatus('playing');

					// Close WS; MSE is no longer needed
					if (this.ws) { this.ws.close(); this.ws = null; }
				}, { once: true });
			} else if (pc.connectionState === 'failed' || pc.connectionState === 'disconnected') {
				pc.close();
				if (this.pc === pc) {
					this.pc = null;
					this.pcActive = false;
					// Reconnect from scratch
					this.setMode(null);
					this.setStatus('error');
					this.clearReconnect();
					this.reconnectTid = setTimeout(() => { this.reconnectTid = null; this.connect(); }, 3000);
				}
			}
		});

		this.onmessage['webrtc'] = (msg) => {
			switch (msg.type) {
				case 'webrtc/candidate':
					pc.addIceCandidate({ candidate: msg.value, sdpMid: '0' }).catch(() => {});
					break;
				case 'webrtc/answer':
					pc.setRemoteDescription({ type: 'answer', sdp: msg.value }).catch(() => {});
					break;
				case 'error':
					if (msg.value.includes('webrtc/offer')) pc.close();
					break;
			}
		};

		pc.addTransceiver('video', { direction: 'recvonly' });
		pc.addTransceiver('audio', { direction: 'recvonly' });
		pc.createOffer()
			.then((offer) => pc.setLocalDescription(offer).then(() => offer))
			.then((offer) => {
				this.ws?.send(JSON.stringify({ type: 'webrtc/offer', value: offer.sdp }));
			})
			.catch(() => {});
	}

	// ── MJPEG ─────────────────────────────────────────────────────────────────

	private startMJPEG(): void {
		this.ondata = (data: ArrayBuffer) => {
			const bytes = new Uint8Array(data);
			let bin = '';
			for (let i = 0; i < bytes.byteLength; i++) bin += String.fromCharCode(bytes[i]);
			if (this.videoEl) {
				this.videoEl.poster   = 'data:image/jpeg;base64,' + btoa(bin);
				this.videoEl.controls = false;
			}
		};
		this.ws?.send(JSON.stringify({ type: 'mjpeg' }));
		this.setMode('mjpeg');
		this.setStatus('playing');
	}
}
