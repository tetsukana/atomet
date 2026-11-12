/**
 * Constellation stick figures, projected onto the 640×360 camera frame using
 * the plate-solve result (RA/Dec/Roll) and the lens profile the solver fitted
 * (radial polynomial; pinhole when none is available), with the optical axis
 * at the image center. Lines are sampled along great circles, so they curve
 * the way the lens does.
 *
 * Roll convention (crates/solver): a sky point at position angle PA (measured
 * from north toward east) and angular distance θ from the pointing center lands
 * at   px = r(θ)·sin(PA + roll),   py = r(θ)·cos(PA + roll)
 * relative to the image center (x right, y down).
 */

export const FRAME_W = 640;
export const FRAME_H = 360;
/** Must match FOCAL_LENGTH_PX in crates/atometd/src/solve.rs */
export const FOCAL_LENGTH_PX = 344;

/** Bright stars used by the figures, J2000 [RA°, Dec°]. */
const S: Record<string, [number, number]> = {
	// Cassiopeia
	Caph: [2.295, 59.150], Schedar: [10.127, 56.537], GammaCas: [14.177, 60.717],
	Ruchbah: [21.454, 60.235], Segin: [28.599, 63.670],
	// Cepheus
	Alderamin: [319.645, 62.586], Alfirk: [322.165, 70.561], Errai: [354.837, 77.633],
	IotaCep: [343.819, 66.200], ZetaCep: [332.714, 58.201],
	// Ursa Major (Big Dipper)
	Dubhe: [165.932, 61.751], Merak: [165.460, 56.382], Phecda: [178.457, 53.695],
	Megrez: [183.857, 57.033], Alioth: [193.507, 55.960], Mizar: [200.981, 54.925],
	Alkaid: [206.885, 49.313],
	// Ursa Minor
	Polaris: [37.955, 89.264], Yildun: [263.054, 86.586], EpsUMi: [251.493, 82.037],
	ZetaUMi: [236.015, 77.795], Kochab: [222.676, 74.156], Pherkad: [230.182, 71.834],
	EtaUMi: [244.376, 75.755],
	// Cygnus
	Deneb: [310.358, 45.280], Sadr: [305.557, 40.257], Gienah: [311.553, 33.970],
	DeltaCyg: [296.244, 45.131], Albireo: [292.680, 27.960],
	// Lyra
	Vega: [279.235, 38.784], Sheliak: [282.520, 33.363], Sulafat: [284.736, 32.690],
	DeltaLyr: [283.626, 36.899], ZetaLyr: [281.193, 37.605],
	// Aquila
	Altair: [297.696, 8.868], Tarazed: [296.565, 10.613], Alshain: [298.828, 6.407],
	// Perseus
	Mirfak: [51.081, 49.861], Algol: [47.042, 40.956], Atik: [55.731, 31.884],
	EpsPer: [59.463, 40.010], DeltaPer: [55.731, 47.787], GammaPer: [46.199, 53.506],
	// Auriga
	Capella: [79.172, 45.998], Menkalinan: [89.882, 44.947], Mahasim: [89.930, 37.213],
	Hassaleh: [74.248, 33.166], Elnath: [81.573, 28.608],
	// Orion
	Betelgeuse: [88.793, 7.407], Rigel: [78.634, -8.202], Bellatrix: [81.283, 6.350],
	Saiph: [86.939, -9.670], Alnitak: [85.190, -1.943], Alnilam: [84.054, -1.202],
	Mintaka: [83.002, -0.299], Meissa: [83.784, 9.934],
	// Taurus
	Aldebaran: [68.980, 16.509], ZetaTau: [84.411, 21.143], Alcyone: [56.871, 24.105],
	GammaTau: [64.948, 15.628],
	// Gemini
	Castor: [113.650, 31.888], Pollux: [116.329, 28.026], Alhena: [99.428, 16.399],
	// Leo
	Regulus: [152.093, 11.967], Denebola: [177.265, 14.572], AlgiebaLeo: [154.993, 19.842],
	Zosma: [168.527, 20.524], EtaLeo: [151.833, 16.763],
	// Bootes / Corona Borealis
	Arcturus: [213.915, 19.182], Izar: [221.247, 27.074], Nekkar: [225.486, 40.391],
	Seginus: [218.019, 38.308],
	// Pegasus (Great Square) + Andromeda
	Markab: [346.190, 15.205], Scheat: [345.944, 28.083], Algenib: [3.309, 15.184],
	Alpheratz: [2.097, 29.091], Mirach: [17.433, 35.621], Almach: [30.975, 42.330],
	// Scorpius
	Antares: [247.352, -26.432], Dschubba: [240.083, -22.622], Graffias: [241.359, -19.805],
	Shaula: [263.402, -37.104], Sargas: [264.330, -42.998], EpsSco: [252.541, -34.293],
	// Southern Cross
	Acrux: [186.650, -63.099], Mimosa: [191.930, -59.689], Gacrux: [187.791, -57.113],
	DeltaCru: [183.786, -58.749],
};

export interface Constellation {
	name: string;
	/** Polylines: each is a chain of star names in `S`. */
	lines: string[][];
}

export const CONSTELLATIONS: Constellation[] = [
	{ name: 'Cassiopeia', lines: [['Caph', 'Schedar', 'GammaCas', 'Ruchbah', 'Segin']] },
	{ name: 'Cepheus', lines: [['Alderamin', 'Alfirk', 'Errai', 'IotaCep', 'ZetaCep', 'Alderamin'], ['Alfirk', 'IotaCep']] },
	{ name: 'Ursa Major', lines: [['Dubhe', 'Merak', 'Phecda', 'Megrez', 'Alioth', 'Mizar', 'Alkaid'], ['Dubhe', 'Megrez']] },
	{ name: 'Ursa Minor', lines: [['Polaris', 'Yildun', 'EpsUMi', 'ZetaUMi', 'Kochab', 'Pherkad', 'EtaUMi', 'ZetaUMi']] },
	{ name: 'Cygnus', lines: [['Deneb', 'Sadr', 'Albireo'], ['DeltaCyg', 'Sadr', 'Gienah']] },
	{ name: 'Lyra', lines: [['Vega', 'ZetaLyr', 'DeltaLyr', 'Sulafat', 'Sheliak', 'ZetaLyr']] },
	{ name: 'Aquila', lines: [['Tarazed', 'Altair', 'Alshain']] },
	{ name: 'Perseus', lines: [['GammaPer', 'Mirfak', 'DeltaPer', 'EpsPer', 'Atik'], ['Mirfak', 'Algol']] },
	{ name: 'Auriga', lines: [['Capella', 'Menkalinan', 'Mahasim', 'Elnath', 'Hassaleh', 'Capella']] },
	{ name: 'Orion', lines: [
		['Betelgeuse', 'Alnitak', 'Saiph'], ['Bellatrix', 'Mintaka', 'Rigel'],
		['Alnitak', 'Alnilam', 'Mintaka'], ['Betelgeuse', 'Meissa', 'Bellatrix'],
	] },
	{ name: 'Taurus', lines: [['Elnath', 'ZetaTau'], ['Aldebaran', 'GammaTau', 'Alcyone']] },
	{ name: 'Gemini', lines: [['Castor', 'Pollux'], ['Pollux', 'Alhena']] },
	{ name: 'Leo', lines: [['Regulus', 'EtaLeo', 'AlgiebaLeo', 'Zosma', 'Denebola', 'Regulus']] },
	{ name: 'Bootes', lines: [['Arcturus', 'Izar', 'Seginus', 'Nekkar']] },
	{ name: 'Pegasus', lines: [['Markab', 'Scheat', 'Alpheratz', 'Algenib', 'Markab']] },
	{ name: 'Andromeda', lines: [['Alpheratz', 'Mirach', 'Almach']] },
	{ name: 'Scorpius', lines: [['Graffias', 'Dschubba', 'Antares', 'EpsSco', 'Sargas', 'Shaula']] },
	{ name: 'Crux', lines: [['Acrux', 'Gacrux'], ['Mimosa', 'DeltaCru']] },
];

export interface LensProfile {
	/** px per radian on-axis */
	f: number;
	/** r = f·(θ + c3·θ³ + c5·θ⁵). Both absent → pinhole, r = f·tanθ. */
	c3?: number;
	c5?: number;
}

export interface ProjectedConstellation {
	name: string;
	/** Polylines in 640×360 px (great-circle arcs sampled finely enough to
	 *  follow lens distortion). A path breaks where a point leaves the lens range. */
	paths: [number, number][][];
	/** Label anchor (px) — mean of the visible vertices — or null if not in frame. */
	label: [number, number] | null;
}

const D2R = Math.PI / 180;
/** Skip points further than this from the optical axis (tan blows up). */
const THETA_LIMIT = 80 * D2R;
/** Sample great circles at least this finely. */
const STEP = 2 * D2R;

type V3 = [number, number, number];
const dot = (a: V3, b: V3) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const cross = (a: V3, b: V3): V3 => [
	a[1] * b[2] - a[2] * b[1],
	a[2] * b[0] - a[0] * b[2],
	a[0] * b[1] - a[1] * b[0],
];
const unit = (a: V3): V3 => {
	const n = Math.hypot(a[0], a[1], a[2]);
	return [a[0] / n, a[1] / n, a[2] / n];
};
const radec = (raDeg: number, decDeg: number): V3 => {
	const ra = raDeg * D2R, dec = decDeg * D2R;
	return [Math.cos(dec) * Math.cos(ra), Math.cos(dec) * Math.sin(ra), Math.sin(dec)];
};

/** Radius in px of a source θ radians off-axis. */
function radius(theta: number, lens: LensProfile): number {
	if (lens.c3 === undefined || lens.c5 === undefined) return lens.f * Math.tan(theta);
	return lens.f * theta * (1 + theta * theta * (lens.c3 + theta * theta * lens.c5));
}

/**
 * Project every constellation for the given pointing (all angles in degrees).
 * `lens` is the solver's fitted profile; without it a pinhole of
 * FOCAL_LENGTH_PX is assumed. Same geometry as crates/solver/src/calib.rs:
 * a point at position angle PA (north → east) and distance θ from the
 * pointing lands at (r·sin(PA+roll), r·cos(PA+roll)) from the image center.
 */
export function projectConstellations(
	raDeg: number, decDeg: number, rollDeg: number,
	lens: LensProfile = { f: FOCAL_LENGTH_PX },
): ProjectedConstellation[] {
	const roll = rollDeg * D2R;
	const p = radec(raDeg, decDeg);
	// North / east axes at the pointing (same construction as compute_roll).
	let n: V3 = [-p[2] * p[0], -p[2] * p[1], 1 - p[2] * p[2]];
	n = Math.hypot(...n) > 1e-9 ? unit(n) : unit([1 - p[0] * p[0], -p[0] * p[1], -p[0] * p[2]]);
	const e = unit(cross(n, p));

	// Returns null beyond the lens range.
	const project = (v: V3): [number, number] | null => {
		const c = dot(v, p);
		if (c < Math.cos(THETA_LIMIT)) return null;
		const theta = Math.acos(Math.min(1, c));
		const phi = Math.atan2(dot(v, e), dot(v, n)) + roll;
		const r = radius(theta, lens);
		return [FRAME_W / 2 + r * Math.sin(phi), FRAME_H / 2 + r * Math.cos(phi)];
	};

	/** Great-circle arc a→b as polyline pieces (split where projection fails). */
	const arc = (a: V3, b: V3): [number, number][][] => {
		const ang = Math.acos(Math.max(-1, Math.min(1, dot(a, b))));
		const k = Math.max(1, Math.ceil(ang / STEP));
		const sin = Math.sin(ang);
		const pieces: [number, number][][] = [];
		let cur: [number, number][] = [];
		for (let i = 0; i <= k; i++) {
			const t = i / k;
			const wa = sin > 1e-9 ? Math.sin((1 - t) * ang) / sin : 1 - t;
			const wb = sin > 1e-9 ? Math.sin(t * ang) / sin : t;
			const pt = project(unit([
				a[0] * wa + b[0] * wb, a[1] * wa + b[1] * wb, a[2] * wa + b[2] * wb,
			]));
			if (pt) cur.push(pt);
			else if (cur.length) { pieces.push(cur); cur = []; }
		}
		if (cur.length) pieces.push(cur);
		return pieces.filter((c) => c.length >= 2);
	};

	const vec = new Map<string, V3>();
	const star = (name: string): V3 | null => {
		if (!vec.has(name) && S[name]) vec.set(name, radec(S[name][0], S[name][1]));
		return vec.get(name) ?? null;
	};

	const out: ProjectedConstellation[] = [];
	for (const c of CONSTELLATIONS) {
		const paths: [number, number][][] = [];
		let sx = 0, sy = 0, cnt = 0;
		for (const line of c.lines) {
			for (let i = 0; i + 1 < line.length; i++) {
				const a = star(line[i]), b = star(line[i + 1]);
				if (!a || !b) continue;
				// Label from the endpoints only, and only if both are visible.
				const pa = project(a), pb = project(b);
				if (pa && pb) {
					sx += pa[0] + pb[0];
					sy += pa[1] + pb[1];
					cnt += 2;
				}
				paths.push(...arc(a, b));
			}
		}
		if (paths.length === 0) continue;
		let label: [number, number] | null = null;
		if (cnt > 0) {
			const lx = sx / cnt, ly = sy / cnt;
			// Only label when the centroid is inside the frame (with a small margin).
			if (lx > 8 && lx < FRAME_W - 8 && ly > 8 && ly < FRAME_H - 8) label = [lx, ly];
		}
		out.push({ name: c.name, paths, label });
	}
	return out;
}
