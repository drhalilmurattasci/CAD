// I-35 bootstrap script.
//
// Rotates the attached entity around its local Y axis at a constant
// rate. Chosen as the smoke test because it exercises three pieces of
// the scripting contract at once:
//
//   - `rot_y` round-trips through `Transform.rotation` (Euler XYZ).
//   - `DT` is the per-tick delta exposed by the host.
//   - the script is a single line, so any corruption in the scope
//     binding shows up as "cube didn't move" without ambiguity.
//
// The 1.5 rad/sec rate is fast enough to be obviously animated even at
// 30 fps and slow enough that motion-sickness-adjacent authors can
// still tell when the rotation axis is wrong.

rot_y = rot_y + 1.5 * DT;
