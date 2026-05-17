import { describe, it, expect } from 'vitest'

// Smoke test for the Vitest bootstrap (added in the pre-Wave-3 intervention
// for itr#312). Proves the harness wires up correctly:
//   - vitest config resolves
//   - jsdom environment loads
//   - setupTests.ts side-effect import registers jest-dom matchers
//   - TypeScript can find vitest types
//
// Safe to delete once any real test exists in this tree (itr#312 will add
// useAuthProfile/usePasskey/Login.tsx tests that supersede this one).
describe('vitest bootstrap', () => {
  it('runs', () => {
    expect(1 + 1).toBe(2)
  })

  it('has a jsdom document', () => {
    expect(typeof document).toBe('object')
    expect(document.body).toBeDefined()
  })

  it('extends expect with jest-dom matchers', () => {
    const el = document.createElement('div')
    el.textContent = 'hello'
    document.body.appendChild(el)
    expect(el).toBeInTheDocument()
    expect(el).toHaveTextContent('hello')
    document.body.removeChild(el)
  })
})
