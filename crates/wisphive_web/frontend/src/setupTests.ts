// Extends Vitest's `expect` with @testing-library/jest-dom matchers
// (toBeInTheDocument, toHaveTextContent, toHaveAttribute, ...). The
// `/vitest` subpath registers cleanup automatically against Vitest's
// lifecycle, so RTL's rendered DOM is torn down between tests.
import '@testing-library/jest-dom/vitest'
