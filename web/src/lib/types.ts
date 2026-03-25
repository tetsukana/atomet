export interface SysStat {
	cpu: number;       // %
	mem_used: number;  // kB
	mem_total: number; // kB
	uptime: number;    // seconds
}

export interface IspStat {
	ae_mode: number;
	it: number;
	ag: number;
	ag_i: number;
	sdg: number;
	idg: number;
	idg_i: number;
	max_it: number;
	max_ag: number;
	max_sdg: number;
	max_idg: number;
	min_it: number;
	min_ag: number;
	min_sdg: number;
	min_idg: number;
	expr_us: number;   // exposure time (µs)
	again: number;     // analog gain (ISP fixed-point)
	dgain: number;     // digital gain (ISP fixed-point)
	fps_actual: number;
	histogram: number[];
}

/** Detection debug frame broadcast from atometd */
export interface DetectionDebug {
	f: number;                            // frame index
	b: [number, number, number, number][]; // blobs [x,y,w,h] in cell coords (×8 → px)
	l: [number, number, number, number][]; // lines [x0,y0,x1,y1] in 640×360 px
	t: [number, number, number, number][]; // tracks [id,mx,my,cnt] in 640×360 px
	c: number[];                           // confirmed track IDs this frame
	mzs: number;                           // max spatial MAD z-score this frame
	mzt: number;                           // max temporal mean/stddev z-score this frame
}

export interface AppState {
	night_mode: boolean;
	ircut_on: boolean;
	led_on: boolean;
	irled_on: boolean;
	flip: [boolean, boolean];
	fps: number;
	record_enabled: boolean;
	detection_enabled: boolean;
	auto_daynight: boolean;
	show_timestamp: boolean;
	show_watermark: boolean;
	timestamp_position: number;
	ae_enable: boolean;
	/** Manual exposure in microseconds. 0 = auto AE. */
	exposure_us: number;
	/** Manual analog gain. 0 = auto. */
	analog_gain: number;
	/** Manual digital gain. 0 = auto. */
	digital_gain: number;
}

export interface MeteorEvent {
	id: number;
	speed: number;        // pixels/frame
	frames: number;       // detection duration in frames
	first_frame: number;
	last_frame: number;
	trajectory: [number, number][];  // [x,y] in 640×360 px
	ts: string;           // timestamp string (YYYYMMDD_HHMMSS)
}
