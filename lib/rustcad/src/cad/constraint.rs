//! Nonlinear constraint solver — the SolveSpace layer.
//!
//! A constraint is anything that reduces to "this expression of the
//! parameter vector should be zero." Points-coincident, distance,
//! parallelism, perpendicularity — every sketch constraint the user
//! places adds one or more scalar residuals to the system. The
//! solver's job is to drive all residuals to zero simultaneously by
//! iterating the parameter vector.
//!
//! Implementation status:
//!
//! - [`Constraint`] trait + the built-in set ([`Distance`],
//!   [`Coincident`], [`ParallelX`], [`Fixed`], [`Equal`]) — done.
//! - [`solve_gauss_newton`] — real, works for ≤30-variable problems.
//!   Uses finite-difference Jacobians and Gaussian-elimination
//!   normal-equation solve, which is plenty for a single sketch but
//!   will want swapping for LM with sparse Jacobians once sketches
//!   grow past ~100 dof.
//! - Robust handling of redundant / conflicting constraints is
//!   out-of-scope for now — the solver reports [`SolveResult::Diverged`]
//!   and leaves conflict diagnosis to the caller.

use thiserror::Error;

/// A scalar constraint over a shared parameter vector.
///
/// Each `Constraint` emits one residual per call to
/// [`residual`](Constraint::residual). Multi-residual constraints
/// (e.g. "two points coincident" → dx + dy) should decompose into
/// multiple `Constraint` objects so the solver can treat them
/// uniformly.
pub trait Constraint {
    /// Human-readable label. Used by error messages, undo-stack UI,
    /// and debug logs.
    fn label(&self) -> &'static str;

    /// Evaluate the residual. The solver drives this to zero.
    fn residual(&self, vars: &[f64]) -> f64;
}

/// Distance-between-two-points constraint. Treats two consecutive
/// `(x, y)` pairs of the parameter vector as points.
#[derive(Debug, Clone, Copy)]
pub struct Distance {
    /// Variable index of the first point's x coordinate.
    pub p1_x:   usize,
    /// Variable index of the first point's y coordinate.
    pub p1_y:   usize,
    /// Variable index of the second point's x coordinate.
    pub p2_x:   usize,
    /// Variable index of the second point's y coordinate.
    pub p2_y:   usize,
    /// Target distance between the two points.
    pub target: f64,
}

impl Constraint for Distance {
    fn label(&self) -> &'static str {
        "distance"
    }

    fn residual(&self, vars: &[f64]) -> f64 {
        let dx = vars[self.p2_x] - vars[self.p1_x];
        let dy = vars[self.p2_y] - vars[self.p1_y];
        (dx * dx + dy * dy).sqrt() - self.target
    }
}

/// Coincident-in-a-single-axis constraint. Two `Coincident { a, b }`
/// (one for x, one for y) together pin two 2D points to the same
/// location.
#[derive(Debug, Clone, Copy)]
pub struct Coincident {
    /// Variable index of the first operand.
    pub a: usize,
    /// Variable index of the second operand.
    pub b: usize,
}

impl Constraint for Coincident {
    fn label(&self) -> &'static str {
        "coincident"
    }

    fn residual(&self, vars: &[f64]) -> f64 {
        vars[self.a] - vars[self.b]
    }
}

/// Value-equality constraint — shorthand for [`Coincident`] when the
/// intent isn't point-coincidence but "these two scalars must agree"
/// (e.g. "these two edges have equal length"). Same residual shape;
/// distinct label for nicer UI.
#[derive(Debug, Clone, Copy)]
pub struct Equal {
    /// Variable index of the first scalar.
    pub a: usize,
    /// Variable index of the second scalar.
    pub b: usize,
}

impl Constraint for Equal {
    fn label(&self) -> &'static str {
        "equal"
    }

    fn residual(&self, vars: &[f64]) -> f64 {
        vars[self.a] - vars[self.b]
    }
}

/// Fix a variable to a specific value. Useful for anchoring a
/// sketch origin or locking a dimension.
#[derive(Debug, Clone, Copy)]
pub struct Fixed {
    /// Variable index to pin.
    pub var:   usize,
    /// Target value.
    pub value: f64,
}

impl Constraint for Fixed {
    fn label(&self) -> &'static str {
        "fixed"
    }

    fn residual(&self, vars: &[f64]) -> f64 {
        vars[self.var] - self.value
    }
}

/// Force a 2D line to be horizontal (parallel to the X axis). Takes
/// the y-indices of the two endpoints.
#[derive(Debug, Clone, Copy)]
pub struct ParallelX {
    /// Variable index of endpoint A's y coordinate.
    pub a_y: usize,
    /// Variable index of endpoint B's y coordinate.
    pub b_y: usize,
}

impl Constraint for ParallelX {
    fn label(&self) -> &'static str {
        "horizontal"
    }

    fn residual(&self, vars: &[f64]) -> f64 {
        vars[self.a_y] - vars[self.b_y]
    }
}

/// Force a 2D line to be vertical (parallel to the Y axis).
#[derive(Debug, Clone, Copy)]
pub struct ParallelY {
    /// Variable index of endpoint A's x coordinate.
    pub a_x: usize,
    /// Variable index of endpoint B's x coordinate.
    pub b_x: usize,
}

impl Constraint for ParallelY {
    fn label(&self) -> &'static str {
        "vertical"
    }

    fn residual(&self, vars: &[f64]) -> f64 {
        vars[self.a_x] - vars[self.b_x]
    }
}

/// Knobs for the Gauss-Newton solver. Defaults are tuned for
/// sketch-sized problems (≤30 dof, millimeter scale).
#[derive(Debug, Clone, Copy)]
pub struct SolverConfig {
    /// Hard iteration cap. Non-convergence → [`SolveResult::Diverged`].
    pub max_iter:    usize,
    /// Convergence threshold on the residual 2-norm.
    pub tolerance:   f64,
    /// Finite-difference step for Jacobian columns.
    pub fd_step:     f64,
    /// Damping factor applied to each step (0.0 = full Newton,
    /// larger = more conservative).
    pub damping:     f64,
    /// Regularization term added to the diagonal of `J^T J` to
    /// stabilize rank-deficient systems.
    pub tikhonov:    f64,
}

impl Default for SolverConfig {
    fn default() -> Self {
        Self {
            max_iter:  64,
            tolerance: 1e-8,
            fd_step:   1e-6,
            damping:   0.0,
            tikhonov:  1e-9,
        }
    }
}

/// Outcome of a [`solve_gauss_newton`] run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SolveResult {
    /// Residual norm fell below [`SolverConfig::tolerance`]. The
    /// variable vector has been updated in place.
    Converged {
        /// Number of Gauss-Newton steps taken.
        iterations:    usize,
        /// Final residual 2-norm.
        residual_norm: f64,
    },
    /// Hit [`SolverConfig::max_iter`] without converging.
    Diverged {
        /// Last residual 2-norm observed.
        residual_norm: f64,
    },
    /// The normal-equation linear system was singular — usually
    /// means the constraint set is under-determined.
    Singular,
}

/// Error raised when solver inputs are invalid (empty constraint
/// set, mismatched lengths, …).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SolverError {
    /// No constraints to solve. Caller probably forgot to populate
    /// the constraint list.
    #[error("solver called with no constraints")]
    NoConstraints,
}

/// Gauss-Newton solver over a slice of boxed constraints.
///
/// Fills `vars` in place with the converged parameter vector.
/// Jacobian is approximated with one-sided finite differences —
/// cheap, but assumes residuals are smooth. For 30-dof-or-less
/// sketches this terminates in a few iterations on a typical
/// desktop CPU.
pub fn solve_gauss_newton(
    constraints: &[Box<dyn Constraint>],
    vars: &mut Vec<f64>,
    config: &SolverConfig,
) -> Result<SolveResult, SolverError> {
    if constraints.is_empty() {
        return Err(SolverError::NoConstraints);
    }

    let m = constraints.len();
    let n = vars.len();

    for iter in 0..config.max_iter {
        let residuals: Vec<f64> = constraints.iter().map(|c| c.residual(vars)).collect();
        let norm = residuals.iter().map(|r| r * r).sum::<f64>().sqrt();
        if norm < config.tolerance {
            return Ok(SolveResult::Converged {
                iterations:    iter,
                residual_norm: norm,
            });
        }

        // Finite-difference Jacobian: J[i][j] = ∂r_i / ∂var_j.
        let mut jac = vec![vec![0.0; n]; m];
        for j in 0..n {
            let saved = vars[j];
            vars[j] = saved + config.fd_step;
            for (i, c) in constraints.iter().enumerate() {
                let r_plus = c.residual(vars);
                jac[i][j] = (r_plus - residuals[i]) / config.fd_step;
            }
            vars[j] = saved;
        }

        // Normal equations: (J^T J + λI) dx = -J^T r.
        let mut jtj = vec![vec![0.0; n]; n];
        let mut jtr = vec![0.0; n];
        for i in 0..m {
            for j in 0..n {
                jtr[j] += jac[i][j] * residuals[i];
                for k in 0..n {
                    jtj[j][k] += jac[i][j] * jac[i][k];
                }
            }
        }
        for j in 0..n {
            jtj[j][j] += config.tikhonov;
            jtr[j] = -jtr[j];
        }

        let dx = match solve_linear_system(&mut jtj, &mut jtr) {
            Some(dx) => dx,
            None => return Ok(SolveResult::Singular),
        };

        let scale = 1.0 - config.damping;
        for j in 0..n {
            vars[j] += dx[j] * scale;
        }
    }

    let final_norm = constraints
        .iter()
        .map(|c| c.residual(vars).powi(2))
        .sum::<f64>()
        .sqrt();
    Ok(SolveResult::Diverged {
        residual_norm: final_norm,
    })
}

/// Dense Gaussian elimination with partial pivoting. `a` is the
/// square coefficient matrix and `b` the right-hand side; both are
/// consumed. Returns `None` if the matrix is singular.
///
/// Kept package-private because it's trivially small — the solver
/// only needs it for ≤30×30 systems, so we don't bother with a
/// full-featured linalg crate.
fn solve_linear_system(a: &mut [Vec<f64>], b: &mut [f64]) -> Option<Vec<f64>> {
    let n = b.len();
    for i in 0..n {
        // Pivot: find the row with the largest |a[k][i]|, k >= i.
        let mut pivot = i;
        for k in (i + 1)..n {
            if a[k][i].abs() > a[pivot][i].abs() {
                pivot = k;
            }
        }
        if a[pivot][i].abs() < 1e-14 {
            return None;
        }
        if pivot != i {
            a.swap(i, pivot);
            b.swap(i, pivot);
        }
        // Eliminate below.
        for k in (i + 1)..n {
            let factor = a[k][i] / a[i][i];
            for j in i..n {
                a[k][j] -= factor * a[i][j];
            }
            b[k] -= factor * b[i];
        }
    }
    // Back-substitute.
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut sum = b[i];
        for j in (i + 1)..n {
            sum -= a[i][j] * x[j];
        }
        x[i] = sum / a[i][i];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_constraint_converges() {
        // Vars: [p1.x, p1.y, p2.x, p2.y], anchor p1 at origin.
        let mut vars = vec![0.0, 0.0, 1.5, 0.2];
        let constraints: Vec<Box<dyn Constraint>> = vec![
            Box::new(Fixed { var: 0, value: 0.0 }),
            Box::new(Fixed { var: 1, value: 0.0 }),
            Box::new(ParallelX { a_y: 1, b_y: 3 }),
            Box::new(Distance {
                p1_x:   0,
                p1_y:   1,
                p2_x:   2,
                p2_y:   3,
                target: 5.0,
            }),
        ];
        let result =
            solve_gauss_newton(&constraints, &mut vars, &SolverConfig::default()).unwrap();
        assert!(matches!(result, SolveResult::Converged { .. }));
        let dx = vars[2] - vars[0];
        let dy = vars[3] - vars[1];
        let length = (dx * dx + dy * dy).sqrt();
        assert!((length - 5.0).abs() < 1e-6);
        assert!(vars[1].abs() < 1e-6);
        assert!(vars[3].abs() < 1e-6);
    }

    #[test]
    fn empty_constraint_list_errors() {
        let mut vars = vec![0.0];
        let err = solve_gauss_newton(&[], &mut vars, &SolverConfig::default()).unwrap_err();
        assert_eq!(err, SolverError::NoConstraints);
    }

    #[test]
    fn fixed_constraint_converges_instantly() {
        let mut vars = vec![7.0];
        let constraints: Vec<Box<dyn Constraint>> = vec![Box::new(Fixed {
            var:   0,
            value: 3.0,
        })];
        let result =
            solve_gauss_newton(&constraints, &mut vars, &SolverConfig::default()).unwrap();
        assert!(matches!(result, SolveResult::Converged { .. }));
        assert!((vars[0] - 3.0).abs() < 1e-6);
    }
}
