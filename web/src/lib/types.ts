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
	record_start_hour: number;
	record_end_hour: number;
	timelapse_enabled: boolean;
	timelapse_start_hour: number;
	timelapse_end_hour: number;
	detection_record_enabled: boolean;
	detection_enabled: boolean;
	detection_start_hour: number;
	detection_end_hour: number;
	solver_enabled: boolean;
	solver_start_hour: number;
	solver_end_hour: number;
	solver_use_twilight: boolean;
	solver_twilight_astro: boolean;
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
	webhook_discord_url: string;
	webhook_slack_url: string;
	webhook_generic_url: string;
	webhook_notify_startup: boolean;
	webhook_notify_detection_start: boolean;
	webhook_notify_detection_end: boolean;
	timezone: string;
	latitude: number;
	longitude: number;
	record_use_twilight: boolean;
	timelapse_use_twilight: boolean;
	detection_use_twilight: boolean;
	record_twilight_astro: boolean;
	timelapse_twilight_astro: boolean;
	detection_twilight_astro: boolean;
	scheduled_reboot_enabled: boolean;
	scheduled_reboot_hour: number;
	scheduled_reboot_minute: number;
	auto_ircut: boolean;
	auto_irled: boolean;
	isp_bypass: number[];
}

export interface MeteorEvent {
	id: number;
	speed: number;        // pixels/frame
	frames: number;       // detection duration in frames
	first_frame: number;
	last_frame: number;
	trajectory: [number, number][];  // [x,y] in 640×360 px
	ts: string;           // timestamp string (YYYYMMDD_HHMMSS)
	short?: boolean;      // single-frame streak detection
	ff?: boolean;         // FF second-pass (σ-clip line) detection
}

export interface FfSavedEvent {
	reason: 'detected' | 'interesting' | 'baseline';
	ts: string;   // YYYYMMDD_HHMMSS_mmm
	path: string; // e.g. /files/ff/2024/01/15/12/FF_*.fits
}

/** Plate solver calibration status, sent once per FF block */
export interface SolverStatus {
	enabled: boolean;
	in_schedule: boolean;
	has_ref: boolean;          // a reference solution exists
	age: number | null;        // seconds since it was last verified
	due: boolean;              // needs (re)calibration / verification
	samples: number;           // agreeing solves collected so far
	need: number;              // solves needed for a reference
	trying: boolean;           // an attempt burst is running
	retry_in: number | null;   // seconds until the next attempt (backing off)
	moved: number | null;      // degrees the last reference moved from the previous one
	/** Locked reference attitude advanced to now (degrees); null = no reference.
	 *  Lets the sky overlays draw without a running solver. */
	pose?: {
		ra: number; dec: number; roll: number;
		lens: { f: number; c3?: number; c5?: number };
	} | null;
}

/** Star extraction result from an FF block's mean frame */
export interface StarsEvent {
	ts?: string;       // YYYYMMDD_HHMMSS (block start, local time)
	count: number;     // confirmed stars (re-sighted across blocks; fed to the solver)
	tentative?: number; // seen too few times so far
	rejected?: number; // static sources (hot pixels / persistent false positives)
	threshold?: number;
	// Automatic plate solve of the confirmed stars; null = no solution / too few stars
	// matched/outer/rms: catalog stars matched after lens calibration (whole frame /
	// beyond 30° from the center) and residual RMS in px
	// lens: radial profile r = f·(θ + c3·θ³ + c5·θ⁵) in px (θ in rad); f·tanθ if c3/c5 are absent
	solve?: {
		ra: number; dec: number; roll: number; votes: number;
		matched: number; outer: number; rms: number;
		lens?: { f: number; c3?: number; c5?: number };
		ms: number;
	} | null;
	// [x,y,flux,state,hits] — sub-pixel position in 640×360 px;
	// state: 0 = confirmed, 1 = static (rejected), 2 = tentative
	stars: number[][];
}
