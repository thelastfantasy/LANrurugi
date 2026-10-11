import { fireEvent, render, screen } from "@testing-library/react"
import { useState } from "react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { Condition } from "@/api/types"
import { FieldRuleEditor } from "@/pages/Settings/FieldRuleEditor"

// The AI entry point's own availability, mocked per test below: the editor reads it from
// `/llm/key-status`, which no unit test has a server for.
const llm = { configured: true }
vi.mock("@/api/hooks", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/api/hooks")>()),
  useLlmKeyStatus: () => ({ data: { configured: llm.configured } }),
}))

// i18next *is* initialized in this file's import graph (TagInput pulls it in), so `t()` returns the
// real English strings rather than the raw keys — the queries below match on those. Other suites in
// this directory see the opposite, hence the differing convention.

const FIELDS = ["title", "posted_at", "rating", "tags", "pages"]

/** Drives the editor the way the settings form does: it owns the condition and feeds it back. */
function Harness({ initial }: { initial?: Condition }) {
  const [condition, setCondition] = useState<Condition | undefined>(initial)
  return (
    <>
      <FieldRuleEditor fields={FIELDS} condition={condition} onChange={setCondition} />
      <pre data-testid="state">{JSON.stringify(condition ?? null)}</pre>
    </>
  )
}

function state(): Condition | null {
  return JSON.parse(screen.getByTestId("state").textContent ?? "null")
}

describe("FieldRuleEditor", () => {
  beforeEach(() => {
    llm.configured = true
  })

  it("offers the AI condition creator only when a model key is configured", () => {
    llm.configured = false
    const { unmount } = render(<Harness />)
    expect(screen.queryByRole("button", { name: "Create with AI" })).toBeNull()
    unmount()

    llm.configured = true
    render(<Harness />)
    expect(screen.getByRole("button", { name: "Create with AI" })).toBeTruthy()
  })

  it("starts with nothing and adds a first condition", () => {
    render(<Harness />)
    expect(state()).toBeNull()

    fireEvent.click(screen.getByRole("button", { name: "Add condition" }))

    const c = state()
    expect(c).toMatchObject({ kind: "all" })
    expect((c as { children: unknown[] }).children).toHaveLength(1)
  })

  it("adds and removes sibling conditions at will", () => {
    render(<Harness initial={{ kind: "all", children: [] }} />)

    const add = () => screen.getByRole("button", { name: "Add condition" })
    fireEvent.click(add())
    fireEvent.click(add())
    expect((state() as { children: unknown[] }).children).toHaveLength(2)

    // Each child carries its own remove control, distinct from the root's "clear all".
    const removes = screen.getAllByRole("button", { name: "Remove" })
    expect(removes).toHaveLength(2)
    fireEvent.click(removes[0])
    expect((state() as { children: unknown[] }).children).toHaveLength(1)
  })

  it("switches a group between all and any", () => {
    render(
      <Harness
        initial={{
          kind: "all",
          children: [
            { kind: "rule", field: "rating", operator: "gte", value: 4 },
            { kind: "rule", field: "pages", operator: "gte", value: 10 },
          ],
        }}
      />,
    )

    // The connective sits between the two conditions it joins, not at the top of the group.
    const connective = screen.getByRole("combobox", { name: "How these conditions combine" })
    fireEvent.change(connective, { target: { value: "any" } })
    expect(state()).toMatchObject({ kind: "any" })

    fireEvent.change(connective, { target: { value: "all" } })
    expect(state()).toMatchObject({ kind: "all" })
  })

  /// With one condition there is nothing to join, so no connective is shown — it appears as soon as a
  /// second condition gives it something to mean.
  it("shows no connective until there are two conditions to join", () => {
    render(
      <Harness
        initial={{
          kind: "all",
          children: [{ kind: "rule", field: "rating", operator: "gte", value: 4 }],
        }}
      />,
    )
    expect(screen.queryByRole("combobox", { name: "How these conditions combine" })).toBeNull()

    fireEvent.click(screen.getByRole("button", { name: "Add condition" }))
    expect(screen.getByRole("combobox", { name: "How these conditions combine" })).toBeTruthy()
  })

  it("inverts a rule into an exclusion and back", () => {
    render(
      <Harness
        initial={{
          kind: "all",
          children: [{ kind: "rule", field: "rating", operator: "gte", value: 4 }],
        }}
      />,
    )

    // The rule's own invert control, not the group's: the group's comes first in the DOM.
    const inverts = screen.getAllByRole("button", { name: "Invert into an exclusion" })
    fireEvent.click(inverts[inverts.length - 1])
    expect(state()).toMatchObject({
      kind: "all",
      children: [{ kind: "not", child: { kind: "rule", field: "rating" } }],
    })

    fireEvent.click(screen.getByRole("button", { name: "Stop excluding" }))
    expect(state()).toMatchObject({
      kind: "all",
      children: [{ kind: "rule", field: "rating" }],
    })
  })

  /// The dead end this guards against: inverting the *root* once left a `not` at the top, which has
  /// no all/any control and no way to add a sibling — the condition could not be edited further.
  it("keeps all/any and add reachable after the root is inverted", () => {
    render(
      <Harness
        initial={{
          kind: "not",
          child: { kind: "rule", field: "rating", operator: "gte", value: 4 },
        }}
      />,
    )

    // The root is shown as a group despite the stored condition being a bare `not`, so a sibling can
    // still be added — which is what was unreachable before.
    fireEvent.click(screen.getByRole("button", { name: "Add condition" }))
    expect((state() as { children: unknown[] }).children).toHaveLength(2)

    // And with two children the connective appears and still edits the root.
    const connective = screen.getByRole("combobox", { name: "How these conditions combine" })
    fireEvent.change(connective, { target: { value: "any" } })
    expect(state()).toMatchObject({ kind: "any" })
    // And the root's own control is labelled as clearing everything, not as removing one child.
    expect(screen.getByRole("button", { name: "Clear all conditions" })).toBeTruthy()
  })

  it("nests a group inside a group", () => {
    render(<Harness initial={{ kind: "all", children: [] }} />)

    fireEvent.click(screen.getByRole("button", { name: "Add group" }))
    expect(state()).toMatchObject({
      kind: "all",
      children: [{ kind: "any" }],
    })

    // The nested group keeps its own kind, independent of its parent's.
    expect(state()).toMatchObject({ kind: "all", children: [{ kind: "any" }] })
  })

  it("resets the operator when the field's type changes", () => {
    render(
      <Harness
        initial={{
          kind: "all",
          children: [{ kind: "rule", field: "title", operator: "contains", value: "x" }],
        }}
      />,
    )

    // Switching a text field to a date must not keep `contains`, which the host can only reject.
    const fieldSelect = screen.getAllByRole("combobox")[0]
    fireEvent.change(fieldSelect, { target: { value: "posted_at" } })

    const rule = (state() as { children: Array<{ operator: string; value: unknown }> }).children[0]
    expect(rule.operator).toBe("older_than")
    expect(typeof rule.value).toBe("number")
  })

  it("says an empty group matches nothing rather than letting it look permissive", () => {
    render(<Harness initial={{ kind: "all", children: [] }} />)
    expect(screen.getByText("An empty group matches nothing — add a condition or remove it.")).toBeTruthy()
  })

  it("states plainly when a source reports no filterable fields", () => {
    render(<FieldRuleEditor fields={[]} condition={undefined} onChange={() => {}} />)
    expect(screen.getByText("This source does not report fields that can be filtered on.")).toBeTruthy()
  })
  /// The space was being eaten on every keystroke: the value was split and trimmed as it was typed,
  /// so `big ` became `big` before the next letter arrived and a value containing a space could not
  /// be entered at all. The `tags` field now uses the shared TagInput, which owns its own draft text;
  /// this covers the plain path that every other multi-value field still takes.
  it("lets a space be typed inside a multi-value field", () => {
    render(
      <Harness
        initial={{
          kind: "all",
          children: [{ kind: "rule", field: "category", operator: "in", value: [] }],
        }}
      />,
    )

    const value = screen.getAllByRole("textbox")[0]
    fireEvent.change(value, { target: { value: "Image Set " } })
    expect((value as HTMLInputElement).value).toBe("Image Set ")

    fireEvent.change(value, { target: { value: "Image Set, Manga" } })
    expect((value as HTMLInputElement).value).toBe("Image Set, Manga")

    // The stored rule still holds a well-formed list, whatever the text currently looks like.
    const rule = (state() as { children: Array<{ value: unknown }> }).children[0]
    expect(rule.value).toEqual(["Image Set", "Manga"])
  })

  it("tidies the text once focus leaves", () => {
    render(
      <Harness
        initial={{
          kind: "all",
          children: [{ kind: "rule", field: "category", operator: "in", value: [] }],
        }}
      />,
    )

    const value = screen.getAllByRole("textbox")[0]
    fireEvent.change(value, { target: { value: "a,  b , " } })
    fireEvent.blur(value)
    expect((value as HTMLInputElement).value).toBe("a, b")
  })

  /// Tags get the shared completion input used elsewhere for metadata, rather than a plain field.
  it("offers library tags as suggestions on a tags rule", () => {
    render(
      <Harness
        initial={{
          kind: "all",
          children: [{ kind: "rule", field: "tags", operator: "includes_all", value: [] }],
        }}
      />,
    )
    // TagInput renders legacy's `tagger` markup; a plain input would not.
    expect(document.querySelector(".tagger")).toBeTruthy()
  })

})
