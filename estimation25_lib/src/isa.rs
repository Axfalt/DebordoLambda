//! Runtime dispatch of hot loops to AVX2 / AVX-512 on x86-64, whose baseline (SSE2) makes
//! `floor`/`ceil` libm calls. aarch64 (the Lambda target) needs nothing: NEON is baseline.

/// A hot loop compiled once per instruction set: `run` must be `#[inline(always)]` so it is
/// inlined into the `#[target_feature]` wrappers (a closure cannot be forced to).
pub(crate) trait Kernel {
    type Output;
    fn run(self) -> Self::Output;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Isa {
    Portable,
    #[cfg(target_arch = "x86_64")]
    Avx2,
    #[cfg(target_arch = "x86_64")]
    Avx512,
}

impl Isa {
    pub(crate) fn detect() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            if std::arch::is_x86_feature_detected!("avx512f") {
                return Isa::Avx512;
            }
            if std::arch::is_x86_feature_detected!("avx2") {
                return Isa::Avx2;
            }
        }
        Isa::Portable
    }

    #[inline(always)]
    pub(crate) fn run<K: Kernel>(self, kernel: K) -> K::Output {
        match self {
            #[cfg(target_arch = "x86_64")]
            // SAFETY: `detect` only returns `Avx512` when AVX-512F is available.
            Isa::Avx512 => unsafe { with_avx512(kernel) },
            #[cfg(target_arch = "x86_64")]
            // SAFETY: `detect` only returns `Avx2` when AVX2 is available.
            Isa::Avx2 => unsafe { with_avx2(kernel) },
            Isa::Portable => kernel.run(),
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
fn with_avx512<K: Kernel>(kernel: K) -> K::Output {
    kernel.run()
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
fn with_avx2<K: Kernel>(kernel: K) -> K::Output {
    kernel.run()
}
