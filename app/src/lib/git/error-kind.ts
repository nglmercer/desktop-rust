/**
 * The parsed Git error vocabulary shared by the legacy runner and typed Rust
 * operations. Keep the dugite dependency behind this boundary while callers
 * migrate; do not import the enum from dugite elsewhere.
 */
export { GitError as GitErrorKind } from 'dugite'
