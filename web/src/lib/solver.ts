import type { SolverStatus } from './types';
import { t } from './i18n';

export type SolverLevel = 'ok' | 'work' | 'idle' | 'warn';

export function fmtDuration(s: number): string {
	if (s < 90) return `${Math.round(s)}${t('s')}`;
	if (s < 5400) return `${Math.round(s / 60)}${t('min')}`;
	return `${Math.round(s / 3600)}${t('h')}`;
}

/** One-line calibration status: [text, level]. */
export function solverLine(s: SolverStatus | null): [string, SolverLevel] {
	if (!s) return ['', 'idle'];
	if (!s.enabled) return [t('Off'), 'idle'];
	const progress = `${s.samples}/${s.need}`;
	if (s.moved !== null && s.moved > 0.3 && !s.due) {
		return [`${t('Camera moved?')} ${s.moved.toFixed(1)}°`, 'warn'];
	}
	if (s.has_ref && !s.due) {
		return [`${t('Calibrated')} · ${t('verified')} ${fmtDuration(s.age ?? 0)} ${t('ago')}`, 'ok'];
	}
	const what = s.has_ref ? t('Verifying') : t('Calibrating');
	if (s.trying || s.samples > 0) return [`${what} (${progress})`, 'work'];
	if (!s.in_schedule) return [`${what}: ${t('waiting for night')}`, 'idle'];
	if (s.retry_in !== null) return [`${what}: ${t('retry in')} ${fmtDuration(s.retry_in)}`, 'idle'];
	return [`${what}…`, 'work'];
}

/** RA in degrees → `12h34.5m`. */
export function fmtRa(deg: number): string {
	const h = (((deg % 360) + 360) % 360) / 15;
	const hh = Math.floor(h);
	const mm = (h - hh) * 60;
	return `${hh}h${mm.toFixed(1).padStart(4, '0')}m`;
}

/** Dec in degrees → `+12°34.5′`. */
export function fmtDec(deg: number): string {
	const a = Math.abs(deg);
	const d = Math.floor(a);
	const m = (a - d) * 60;
	return `${deg < 0 ? '−' : '+'}${d}°${m.toFixed(1).padStart(4, '0')}′`;
}
