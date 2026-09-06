import { Input as BaseInput } from "@base-ui/react/input"
import { type ComponentProps, type ReactNode, type TextareaHTMLAttributes,useLayoutEffect, useRef } from "react"

/** Ported from Base UI's own `createChangeEventDetails` (`internals/createBaseUIEventDetails.ts`)
 * — `Input`'s `onValueChange` is really `Field.Control`'s, and `Field.Control` always constructs
 * this exact shape for its second argument. Kept identical so a `multiline` caller's
 * `eventDetails.cancel()`/`allowPropagation()` behave the same as the real `Input`'s. */
function createChangeEventDetails(event: Event): BaseInputChangeEventDetails {
  let canceled = false
  let propagationAllowed = false
  return {
    reason: "none",
    event,
    cancel() {
      canceled = true
    },
    allowPropagation() {
      propagationAllowed = true
    },
    get isCanceled() {
      return canceled
    },
    get isPropagationAllowed() {
      return propagationAllowed
    },
    trigger: undefined,
  }
}

type BaseInputChangeEventDetails = {
  reason: "none"
  event: Event
  cancel: () => void
  allowPropagation: () => void
  isCanceled: boolean
  isPropagationAllowed: boolean
  trigger: Element | undefined
}

type BaseInputProps = ComponentProps<typeof BaseInput>

type SingleLineProps = {
  rows?: undefined
  className?: BaseInputProps["className"]
  style?: React.CSSProperties
} & Omit<BaseInputProps, "className" | "style">

/** Built from `TextareaHTMLAttributes`, not `BaseInputProps` — Base UI's whole event surface
 * (`onClick`, `onCopy`, `onChange`, ...) is typed around `HTMLInputElement`/its own `BaseUIEvent`
 * wrapper, which conflicts with every same-named native `<textarea>` handler, not just
 * `onChange`. `value`/`defaultValue`/`onValueChange` are Base UI's own `onValueChange` contract
 * layered on top — the two prop sets don't otherwise overlap. */
type MultilineProps = {
  rows: number
  className?: string
  style?: React.CSSProperties
  value?: string
  defaultValue?: string
  onValueChange?: (value: string, eventDetails: BaseInputChangeEventDetails) => void
  placeholder?: string
  autoFocus?: boolean
  maxLength?: number
} & Pick<TextareaHTMLAttributes<HTMLTextAreaElement>, "wrap" | "onChange" | "onKeyDown">

/** Site-wide `.stdinput`-styled text input. Passing `rows` (native to `<textarea>`, meaningless
 * on `<input>` — an unambiguous signal, unlike a `style.minHeight` that could mean either)
 * switches the underlying element from Base UI's `Input` (a plain `<input>` — its own docs
 * confirm no textarea composition) to a native `<textarea>` that grows with content past that
 * starting row count. `className` appends onto `"stdinput"` rather than replacing it. */
export function Input(props: SingleLineProps | MultilineProps) {
  if (props.rows !== undefined) {
    const { className, style, value, ...rest } = props
    return <AutoGrowTextarea className={className} style={style} value={value} {...rest} />
  }
  const { className, style, ...rest } = props
  const combinedClassName =
    typeof className === "function"
      ? (state: Parameters<NonNullable<Extract<ComponentProps<typeof BaseInput>["className"], (...args: never[]) => unknown>>>[0]) =>
          `stdinput ${className(state)}`
      : className
        ? `stdinput ${className}`
        : "stdinput"
  return (
    <BaseInput
      className={combinedClassName}
      // Matches `.stdbtn`'s own fixed height so this row's buttons and input line up.
      style={{ height: 21, ...style }}
      {...rest}
    />
  )
}

function AutoGrowTextarea({
  className,
  style,
  value,
  defaultValue,
  onValueChange,
  onChange,
  ...props
}: MultilineProps) {
  const ref = useRef<HTMLTextAreaElement>(null)

  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    el.style.height = "auto"
    el.style.height = `${el.scrollHeight}px`
  }, [value])

  return (
    <textarea
      ref={ref}
      className={className ? `stdinput ${className}` : "stdinput"}
      style={{ height: 21, resize: "none", overflow: "hidden", ...style }}
      value={value as string | undefined}
      defaultValue={defaultValue as string | undefined}
      onChange={(event) => {
        // Same order/short-circuit as `Field.Control`'s own `onChange`: fire `onValueChange`
        // first so a caller's `cancel()`/preventDefault can still be observed, but a controlled
        // `value` only ever changes via the prop, never this handler re-deriving it.
        const details = createChangeEventDetails(event.nativeEvent)
        onValueChange?.(event.currentTarget.value, details)
        onChange?.(event)
      }}
      {...props}
    />
  )
}

/** Chakra-UI-style `InputGroup` — wraps `Input` with `startElement`/`endElement` slots pinned to
 * its edges. `slotSize` (default 26) isn't auto-measured; pass one explicitly for wider content. */
export function InputGroup({
  startElement,
  endElement,
  slotSize = 26,
  style,
  children,
}: {
  startElement?: ReactNode
  endElement?: ReactNode
  slotSize?: number
  style?: React.CSSProperties
  children: ReactNode
}) {
  return (
    <span
      style={{
        position: "relative",
        display: "inline-block",
        ...(startElement ? { ["--input-group-start-width" as string]: `${slotSize}px` } : {}),
        ...(endElement ? { ["--input-group-end-width" as string]: `${slotSize}px` } : {}),
        ...style,
      }}
    >
      {children}
      {startElement && (
        <span
          style={{
            position: "absolute",
            // `Input`'s own `.stdinput` margin shifts its visual box 2px below this span's center.
            top: "calc(50% + 2px)",
            left: 4,
            transform: "translateY(-50%)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            width: slotSize - 8,
            height: slotSize - 8,
            color: "inherit",
          }}
        >
          {startElement}
        </span>
      )}
      {endElement && (
        <span
          style={{
            position: "absolute",
            top: "calc(50% + 2px)",
            right: 4,
            transform: "translateY(-50%)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            width: slotSize - 8,
            height: slotSize - 8,
            color: "inherit",
          }}
        >
          {endElement}
        </span>
      )}
    </span>
  )
}
