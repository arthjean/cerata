pub const KEEP_ALL_SECS: i64 = 24 * 3600;
pub const HOURLY_SECS: i64 = 30 * 24 * 3600;

const HOUR: i64 = 3600;
const DAY: i64 = 24 * 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dated {
    pub ts: i64,
    pub pinned: bool,
}

pub fn kept(versions: &[Dated], now: i64) -> Vec<bool> {
    let mut keep = vec![false; versions.len()];
    let mut last_bucket: Option<(i64, i64)> = None;
    for (index, version) in versions.iter().enumerate().rev() {
        let age = now.saturating_sub(version.ts);
        let bucket = if age < KEEP_ALL_SECS {
            None
        } else if age < HOURLY_SECS {
            Some((HOUR, version.ts.div_euclid(HOUR)))
        } else {
            Some((DAY, version.ts.div_euclid(DAY)))
        };
        let first_in_bucket = match bucket {
            None => true,
            Some(bucket) => last_bucket != Some(bucket),
        };
        if let Some(bucket) = bucket {
            last_bucket = Some(bucket);
        }
        keep[index] = version.pinned || first_in_bucket;
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn at(age: i64) -> Dated {
        Dated {
            ts: NOW - age,
            pinned: false,
        }
    }

    #[test]
    fn everything_of_the_last_day_is_kept() {
        let versions: Vec<Dated> = (0..50).rev().map(|n| at(n * 60)).collect();
        assert!(kept(&versions, NOW).iter().all(|keep| *keep));
    }

    #[test]
    fn the_last_version_of_each_hour_is_kept_up_to_thirty_days() {
        let base = (NOW - 2 * DAY).div_euclid(HOUR) * HOUR;
        let versions = [
            Dated {
                ts: base + 60,
                pinned: false,
            },
            Dated {
                ts: base + 600,
                pinned: false,
            },
            Dated {
                ts: base + 1200,
                pinned: false,
            },
            Dated {
                ts: base + 3000,
                pinned: false,
            },
            Dated {
                ts: base + HOUR + 10,
                pinned: false,
            },
        ];
        assert_eq!(kept(&versions, NOW), [false, false, false, true, true]);
    }

    #[test]
    fn the_last_version_of_each_day_is_kept_beyond_thirty_days() {
        let day = (NOW - 400 * DAY).div_euclid(DAY) * DAY;
        let versions = [
            Dated {
                ts: day + 10,
                pinned: false,
            },
            Dated {
                ts: day + 5 * HOUR,
                pinned: false,
            },
            Dated {
                ts: day + 23 * HOUR,
                pinned: false,
            },
            Dated {
                ts: day + DAY + 1,
                pinned: false,
            },
        ];
        assert_eq!(kept(&versions, NOW), [false, false, true, true]);
    }

    #[test]
    fn a_pinned_version_is_always_kept() {
        let day = (NOW - 90 * DAY).div_euclid(DAY) * DAY;
        let versions = [
            Dated {
                ts: day + 10,
                pinned: true,
            },
            Dated {
                ts: day + 20,
                pinned: false,
            },
            Dated {
                ts: day + 30,
                pinned: false,
            },
        ];
        assert_eq!(kept(&versions, NOW), [true, false, true]);
    }

    #[test]
    fn tiers_meet_at_their_boundaries() {
        let versions = [
            at(HOURLY_SECS + 5),
            at(KEEP_ALL_SECS + 5),
            at(KEEP_ALL_SECS - 5),
        ];
        assert_eq!(kept(&versions, NOW), [true, true, true]);
    }
}
