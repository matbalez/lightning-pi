use num_bigint::BigInt;
use num_traits::Zero;

// Integer interval arithmetic for Machin's formula. Every division contributes
// less than one scale unit of error; the first omitted alternating term adds
// less than one. Return only when BOTH bounds round to the same decimal.
fn atan(scale: &BigInt, denominator: u32) -> (BigInt, u32) {
    let square = denominator * denominator;
    let mut power = BigInt::from(denominator);
    let mut sum = BigInt::zero();
    let mut k = 0u32;
    loop {
        let term = scale / (&power * BigInt::from(2 * k + 1));
        if term.is_zero() {
            return (sum, k + 1);
        }
        if k.is_multiple_of(2) {
            sum += term;
        } else {
            sum -= term;
        }
        power *= square;
        k += 1;
    }
}

pub fn rounded(digits: u32) -> String {
    for guard in (12..=60).step_by(12) {
        let scale = BigInt::from(10u32).pow(digits + guard);
        let (a, ea) = atan(&scale, 5);
        let (b, eb) = atan(&scale, 239);
        let pi = a * 16 - b * 4;
        let error = BigInt::from(16 * ea + 4 * eb);
        let divisor = BigInt::from(10u32).pow(guard);
        let half = &divisor / 2;
        let low: BigInt = (&pi - &error + &half) / &divisor;
        let high: BigInt = (&pi + &error + &half) / &divisor;
        if low == high {
            let mut s = low.to_string();
            if digits > 0 {
                s.insert(s.len() - digits as usize, '.');
            }
            return s;
        }
    }
    // No finite decimal tie exists for irrational pi; the supported 10,000
    // decimal range is covered by tests. Do not return an uncertified answer.
    panic!("rounding interval did not converge")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decimal_rounding() {
        for (d, s) in [
            (0, "3"),
            (1, "3.1"),
            (2, "3.14"),
            (3, "3.142"),
            (4, "3.1416"),
            (5, "3.14159"),
            (10, "3.1415926536"),
            (20, "3.14159265358979323846"),
            (50, "3.14159265358979323846264338327950288419716939937511"),
        ] {
            assert_eq!(rounded(d), s);
        }
    }
    #[test]
    fn max_digits() {
        let s = rounded(10_000);
        assert_eq!(s.len(), 10_002);
        assert!(s.starts_with("3.14159265358979323846264338327950288419716939937510"));
    }
}
