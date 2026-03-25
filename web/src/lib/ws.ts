import type { AppState, SysStat, IspStat, DetectionDebug, MeteorEvent } from './types';

export class AtometWebSocket {
	private ws: WebSocket | null = null;
	private url: string;
	onAppState: ((state: AppState) => void) | null = null;
	onSysStat: ((stat: SysStat) => void) | null = null;
	onIspStat: ((stat: IspStat) => void) | null = null;
	onDetectionDebug: ((debug: DetectionDebug) => void) | null = null;
	onMeteorEvent: ((event: MeteorEvent) => void) | null = null;
	onBinary: ((data: Uint8Array) => void) | null = null;
	onStatusChange: ((connected: boolean) => void) | null = null;
	private reconnectTimer: ReturnType<typeof setTimeout> | null = null;

	constructor() {
		const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
		this.url = `${protocol}//${location.host}/ws`;
	}

	connect() {
		if (this.ws) return;

		this.ws = new WebSocket(this.url);
		this.ws.binaryType = 'arraybuffer';

		this.ws.onopen = () => {
			this.onStatusChange?.(true);
			if (this.reconnectTimer) {
				clearTimeout(this.reconnectTimer);
				this.reconnectTimer = null;
			}
		};

		this.ws.onmessage = (event) => {
			if (event.data instanceof ArrayBuffer) {
				this.onBinary?.(new Uint8Array(event.data));
			} else if (typeof event.data === 'string') {
				try {
					const msg = JSON.parse(event.data);
					if (msg.type === 'appstate') {
						this.onAppState?.(msg.data);
					} else if (msg.type === 'sysstat') {
						this.onSysStat?.(msg.data);
					}else if (msg.type === 'ispstat') {
						this.onIspStat?.(msg.data);
					} else if (msg.type === 'det') {
						this.onDetectionDebug?.(msg as DetectionDebug);
					} else if (msg.type === 'meteor') {
						this.onMeteorEvent?.(msg as MeteorEvent);
					}
				} catch {
					// ignore parse errors
				}
			}
		};

		this.ws.onclose = () => {
			this.ws = null;
			this.onStatusChange?.(false);
			this.scheduleReconnect();
		};

		this.ws.onerror = () => {
			this.ws?.close();
		};
	}

	send(msg: Record<string, unknown>) {
		if (this.ws?.readyState === WebSocket.OPEN) {
			this.ws.send(JSON.stringify(msg));
		}
	}

	disconnect() {
		if (this.reconnectTimer) {
			clearTimeout(this.reconnectTimer);
			this.reconnectTimer = null;
		}
		this.ws?.close();
		this.ws = null;
	}

	private scheduleReconnect() {
		if (!this.reconnectTimer) {
			this.reconnectTimer = setTimeout(() => {
				this.reconnectTimer = null;
				this.connect();
			}, 3000);
		}
	}
}
