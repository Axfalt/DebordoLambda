//! Runtime selection of the widest vector instruction set for the hot loops.
//!
//! Release builds target baseline x86-64 (SSE2), where `f64::floor`/`ceil` are libm calls and
//! 32-bit multiplies vectorise poorly. Hot loops are therefore written as a [`Kernel`] and run
//! through [`Isa::run`], which calls them from a `#[target_feature]` function: because
//! [`Kernel::run`] implementations are `#[inline(always)]`, the loop and the `#[inline(always)]`
//! helpers it uses are compiled for AVX2 or AVX-512 when the CPU supports it. (A closure would not
//! do: stable Rust cannot force it to be inlined into the target-feature wrapper.) On aarch64 (the
//! Lambda target) NEON and the rounding instructions are baseline: the portable path is optimal.

/// A hot loop to compile once per instruction set. Implementations must mark `run`
/// `#[inline(always)]`.
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
    /// Widest supported instruction set (`is_x86_feature_detected!` caches its result).
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

    /// Runs `kernel` compiled with this instruction set's target features.
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
