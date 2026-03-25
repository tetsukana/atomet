//! Database generator for the plate solver.
//!
//! Reads the Hipparcos catalog (`hip_main.dat`) and generates `database.bin`.
//! Run on host: `cargo run -p solver --bin gen`

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
};

use solver::{AttitudeSolution, Database, QUANTIZATION_BINS, float_to_fixed10};
use solver::{Equatorial, HipStar, Vec2, Vec3};

const GOLDEN_RATIO: f32 = 1.6180;
const FOV: f32 = 30.0 * std::f32::consts::PI / 180.0;
const MAG_LIMIT: f32 = 5.0;
const STARS_PER_FOV: usize = 150;
const DB_LATTICE_N: usize = 100_000;
const MAX_PATTERNS_PER_CENTER: usize = 50;
const EPOCH_TARGET: f32 = 2025.0;
const EPOCH_SOURCE: f32 = 1991.25;

struct FibonacciLattice(usize, usize);

impl Iterator for FibonacciLattice {
    type Item = Equatorial;

    fn next(&mut self) -> Option<Self::Item> {
        if self.1 >= self.0 {
            return None;
        }

        let x = ((self.1 as f32) / GOLDEN_RATIO) % 1.;
        let y = self.1 as f32 / self.0 as f32;

        self.1 += 1;

        Some(Equatorial {
            ra: 2. * core::f32::consts::PI * x,
            dec: core::f32::consts::FRAC_PI_2 - f32::acos(1. - 2. * y),
        })
    }
}

fn read_hip_main(reader: BufReader<File>) -> Vec<HipStar> {
    let delta_t = EPOCH_TARGET - EPOCH_SOURCE;

    let mut stars: Vec<HipStar> = Vec::new();
    reader.lines().for_each(|line| {
        let line = line.expect("Could not read line");
        let fields: Vec<&str> = line.split('|').collect();

        let hip_id = match fields[1]
            .split_whitespace()
            .last()
            .unwrap_or_default()
            .parse::<u32>()
        {
            Ok(num) => num,
            Err(_) => return,
        };

        let mag = match fields[5]
            .split_whitespace()
            .last()
            .unwrap_or_default()
            .parse::<f32>()
        {
            Ok(num) => num,
            Err(_) => return,
        };

        if mag > MAG_LIMIT {
            return;
        }

        let ra = match fields[8].parse::<f32>() {
            Ok(num) => num,
            Err(_) => return,
        };

        let dec = match fields[9].parse::<f32>() {
            Ok(num) => num,
            Err(_) => return,
        };

        let pm_ra = match fields[12]
            .split_whitespace()
            .last()
            .unwrap_or_default()
            .parse::<f32>()
        {
            Ok(num) => num,
            Err(_) => return,
        };

        let pm_dec = match fields[13]
            .split_whitespace()
            .last()
            .unwrap_or_default()
            .parse::<f32>()
        {
            Ok(num) => num,
            Err(_) => return,
        };

        // Apply proper motion to target epoch
        let ra = (ra + (pm_ra * delta_t / 3600000.))
            .rem_euclid(360.)
            .to_radians();
        let dec = (dec + (pm_dec * delta_t / 3600000.)).to_radians();

        stars.push(HipStar {
            id: hip_id,
            mag,
            eq: Equatorial { ra, dec },
        });
    });

    stars
}

fn edge_length(points: [Vec3; 4]) -> Vec<(f32, usize, usize)> {
    vec![
        (((points[0] - points[1]).norm() / 2.).asin() * 2., 0, 1),
        (((points[0] - points[2]).norm() / 2.).asin() * 2., 0, 2),
        (((points[0] - points[3]).norm() / 2.).asin() * 2., 0, 3),
        (((points[1] - points[2]).norm() / 2.).asin() * 2., 1, 2),
        (((points[1] - points[3]).norm() / 2.).asin() * 2., 1, 3),
        (((points[2] - points[3]).norm() / 2.).asin() * 2., 2, 3),
    ]
}

fn separation_for_density(fov: f32, stars_per_fov: usize) -> f32 {
    0.6 * fov / (stars_per_fov as f32).sqrt()
}

fn make_key(a: [u8; 5]) -> u32 {
    let mut key: u32 = 0;
    for (i, &v) in a.iter().enumerate() {
        key |= ((v & 0x3F) as u32) << (32 - 6 * (i + 1));
    }
    key
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    key: u32,
    ref_dir: Vec3,
    sky_centroid: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CompressedEntry {
    key: u32,
    compressed: u64,
}

impl CompressedEntry {
    fn new(entry: Entry) -> Self {
        let ref_dir_x = float_to_fixed10(entry.ref_dir.0) as u64;
        let ref_dir_y = float_to_fixed10(entry.ref_dir.1) as u64;
        let ref_dir_z = float_to_fixed10(entry.ref_dir.2) as u64;
        let centroid_x = float_to_fixed10(entry.sky_centroid.0) as u64;
        let centroid_y = float_to_fixed10(entry.sky_centroid.1) as u64;
        let centroid_z = float_to_fixed10(entry.sky_centroid.2) as u64;

        let compressed = ref_dir_x
            | (ref_dir_y << 10)
            | (ref_dir_z << 20)
            | (centroid_x << 30)
            | (centroid_y << 40)
            | (centroid_z << 50);

        Self {
            key: entry.key,
            compressed,
        }
    }
}

fn generate_database(stars: &[HipStar]) {
    let lattice = FibonacciLattice(DB_LATTICE_N, 0);

    let stars_separation = separation_for_density(FOV, STARS_PER_FOV);
    let stars_separation_dist = 2. * (stars_separation / 2.).sin();

    let mut database = File::create("database.bin").unwrap();
    let mut writer = BufWriter::new(&mut database);
    let mut entries = HashSet::new();
    lattice.for_each(|center| {
        let mut stars_in_field: Vec<&HipStar> = stars
            .iter()
            .filter(|&&s| {
                let u = s.eq.to_cartesian();
                center.to_cartesian().angle_between(&u) < FOV / 2.0
            })
            .collect();

        stars_in_field.sort_by(|a, b| a.mag.partial_cmp(&b.mag).unwrap());

        let mut count = 0;
        'outer: for i in 3..stars_in_field.len() {
            for j in 2..i {
                for k in 1..j {
                    for l in 0..k {
                        let stars = [
                            stars_in_field[l],
                            stars_in_field[k],
                            stars_in_field[j],
                            stars_in_field[i],
                        ];
                        let points = [
                            stars[0].eq.to_cartesian(),
                            stars[1].eq.to_cartesian(),
                            stars[2].eq.to_cartesian(),
                            stars[3].eq.to_cartesian(),
                        ];

                        let mut l = edge_length(points);
                        l.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

                        if l.first().unwrap().0 < stars_separation_dist {
                            continue;
                        }

                        let largest_edge = l.last().unwrap();

                        let mut key = [0, 0, 0, 0, 0];
                        for m in 0..5 {
                            let bin = ((l[m].0 / largest_edge.0) * QUANTIZATION_BINS).floor() as u8;
                            key[m] = bin;
                        }
                        let key = make_key(key);

                        let centroid = (points[0] + points[1] + points[2] + points[3]) / 4.;

                        let centroid_distance = [
                            ((points[0] - centroid).norm() / 2.).asin() * 2.,
                            ((points[1] - centroid).norm() / 2.).asin() * 2.,
                            ((points[2] - centroid).norm() / 2.).asin() * 2.,
                            ((points[3] - centroid).norm() / 2.).asin() * 2.,
                        ];

                        let a = if centroid_distance[largest_edge.1]
                            > centroid_distance[largest_edge.2]
                        {
                            points[largest_edge.1]
                        } else {
                            points[largest_edge.2]
                        };

                        let b = if centroid_distance[largest_edge.1]
                            > centroid_distance[largest_edge.2]
                        {
                            points[largest_edge.2]
                        } else {
                            points[largest_edge.1]
                        };

                        let c1 = b - a;

                        entries.insert(CompressedEntry::new(Entry {
                            key,
                            ref_dir: c1,
                            sky_centroid: centroid,
                        }));

                        count += 1;
                        if count == MAX_PATTERNS_PER_CENTER {
                            break 'outer;
                        }
                    }
                }
            }
        }
    });

    let mut keys = HashMap::new();
    entries.iter().for_each(|entry| {
        if let std::collections::hash_map::Entry::Vacant(e) = keys.entry(entry.key) {
            e.insert(vec![entry.compressed]);
        } else {
            keys.get_mut(&entry.key).unwrap().push(entry.compressed);
        }
    });

    let mut max = 0;
    writer
        .write_all(&(keys.len() as u32).to_be_bytes())
        .unwrap();
    let mut flat_array: Vec<u64> = vec![];
    println!("{}", keys.len());
    let mut a1 = vec![];
    for entry in keys {
        let start = flat_array.len() as u32;
        let length = entry.1.len() as u32;

        max = max.max(length);

        flat_array.extend_from_slice(&entry.1);

        let compressed = ((start & 0xFFFFF) << 12) | (length & 0xFFF);
        a1.push((entry.0, compressed));
    }

    writer
        .write_all(&(flat_array.len() as u32).to_be_bytes())
        .unwrap();

    a1.sort_by_key(|a| a.0);

    let lut = create_lut(&a1);

    writer.write_all(&(lut.len() as u32).to_be_bytes()).unwrap();

    writer
        .write_all(&(stars.len() as u32).to_be_bytes())
        .unwrap();

    a1.iter().for_each(|x| {
        writer.write_all(&x.0.to_be_bytes()).unwrap();
    });

    a1.iter().for_each(|x| {
        writer.write_all(&x.1.to_be_bytes()).unwrap();
    });

    println!("max patterns per key: {}", max);

    for &value in &flat_array {
        writer.write_all(&value.to_be_bytes()).unwrap();
    }

    for v in lut {
        writer.write_all(&v.to_be_bytes()).unwrap();
    }

    for star in stars.iter() {
        let vec = star.eq.to_cartesian();
        writer.write_all(&vec.0.to_be_bytes()).unwrap();
        writer.write_all(&vec.1.to_be_bytes()).unwrap();
        writer.write_all(&vec.2.to_be_bytes()).unwrap();
        writer.write_all(&star.mag.to_be_bytes()).unwrap();
    }

    writer.flush().unwrap();
    drop(writer);
    database.flush().unwrap();
}

fn create_lut(a1: &[(u32, u32)]) -> Vec<u32> {
    let mut lut = vec![0u32; 4097];
    let mut current_bucket = 0;

    for (i, &(key, _)) in a1.iter().enumerate() {
        let bucket = ((key >> 20) & 0xFFF) as usize;

        while current_bucket < bucket {
            current_bucket += 1;
            lut[current_bucket] = i as u32;
        }
    }

    while current_bucket < 4096 {
        current_bucket += 1;
        lut[current_bucket] = a1.len() as u32;
    }

    lut
}

const N: Vec3 = Vec3(0., 0., 1.);

/// Gnomonic projection of unit vector `u` onto tangent plane at `v`.
fn tan_projection(v: &Vec3, u: &Vec3) -> Vec2 {
    let y0 = N - *v * N.dot(v);

    let y0 = if y0.norm() < 1e-12 {
        Vec3(1., 0., 0.) - *v * Vec3(1., 0., 0.).dot(v)
    } else {
        y0
    };

    let y = y0.normalize();
    let x = y.cross(v).normalize();

    let s = u.dot(&x) / u.dot(v);
    let t = u.dot(&y) / u.dot(v);
    Vec2(s, t)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() > 1 && args[1] == "generate" {
        let catalog = if args.len() > 2 {
            &args[2]
        } else {
            "hip_main.dat"
        };
        println!("Reading Hipparcos catalog from {catalog}...");
        let file = File::open(catalog).expect("Could not open catalog file");
        let reader = BufReader::new(file);
        let stars = read_hip_main(reader);
        println!("Read {} stars (mag ≤ {MAG_LIMIT})", stars.len());
        generate_database(&stars);
        println!("Database written to database.bin");
        return;
    }

    // Default: test mode — load database and benchmark
    let db_path = if args.len() > 1 {
        &args[1]
    } else {
        "database.bin"
    };
    println!("Loading database from {db_path}...");
    let db = Database::load(db_path);

    let test_fov = FOV;
    let n = DB_LATTICE_N;

    let lattice = FibonacciLattice(n, 0);
    let mut errors = vec![];
    let start = std::time::Instant::now();

    lattice.for_each(|center| {
        let mut stars_in_field = vec![];
        let center_vec = center.to_cartesian();

        db.stars
            .iter()
            .filter(|s| center_vec.angle_between(&s.0) < test_fov / 2.0)
            .for_each(|s| {
                let xy = tan_projection(&center_vec, &s.0);
                stars_in_field.push((xy, s.1));
            });

        stars_in_field.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        let centroids = stars_in_field
            .iter()
            .map(|(xy, _)| (xy.0, xy.1))
            .collect::<Vec<(f32, f32)>>();

        let dot_threshold = 0.5_f32.to_radians().cos();
        let q = db.solve(&centroids, (0., 0.), 1.0, dot_threshold, 100, Some(6));

        if let Some(AttitudeSolution { pointing, .. }) = q {
            let error = (pointing - center_vec).norm().to_degrees();
            if error > 1.0 {
                errors.push(center_vec);
            }
        } else {
            errors.push(center_vec);
        }
    });

    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "Solved {n} positions in {elapsed:.2}s ({:.1} µs/solve)",
        elapsed * 1e6 / n as f64
    );
    println!(
        "Error rate: {:.2}% ({}/{})",
        errors.len() as f64 / n as f64 * 100.,
        errors.len(),
        n
    );
}
