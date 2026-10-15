import { useQueryClient } from "@tanstack/react-query"
import { useState } from "react"
import { useTranslation } from "react-i18next"
import { Navigate, useNavigate, useSearchParams } from "react-router-dom"

import { useChangePassword, useImportLegacyCount, useSettings, useUpdateSettings } from "@/api/hooks"
import { Button } from "@/components/common-ui/Form"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { SESSION_QUERY_KEY } from "@/session/queryKey"
import { useSession } from "@/session/SessionProvider"
import { FONT_SIZE_SM } from "@/theme"
import { toast } from "@/toast"

import { BasicsStep } from "./BasicsStep"
import { LegacyImportStep } from "./LegacyImportStep"
import { LlmKeyStep } from "./LlmKeyStep"
import { FIRST_RUN_WIZARD_STEPS, type FirstRunWizardStep,passwordStepProblem } from "./logic"
import { StepActions, WizardStepNav } from "./WizardStepNav"

/** What the user has typed but not necessarily committed yet. */
interface Drafts {
  password: string
  passwordConfirmation: string
  llmKey: string
  /** `null` = untouched. Nothing is written for an untouched picker: language and timezone always
   *  already have a value (the server's own default), so "unchanged" is not a draft at all — the
   *  same reason a blank password field is a no-op rather than an instruction to clear it. */
  language: string | null
  timezone: string | null
}

/**
 * First-run wizard (`/welcome`).
 *
 * Every item it covers is optional and individually skippable, so the page deliberately has no
 * "you must do this" gate on it — the only thing it persists is that it was *shown*, written when
 * the user leaves through either the finish button or the wizard-wide skip. Without that, the
 * auto-open gate (`FirstRunWizardGate`) would bounce every visit to the library straight back here.
 *
 * Reachable two ways: automatically once per instance, and explicitly from the Settings page's
 * own "run the wizard again" button (`?from=settings`), which only changes where it returns to.
 *
 * **Drafts live here, not in the steps.** Tabs are freely clickable, so a step unmounts the moment
 * you leave it: anything typed there would be lost, and the whole point of a wizard you can wander
 * around is that wandering is safe. Committing them lives here too, because the last page's "submit
 * everything outstanding" button has to run the exact same writes as each step's own Next.
 */
export function FirstRunWizard() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [searchParams] = useSearchParams()
  const updateSettings = useUpdateSettings()
  const changePassword = useChangePassword()
  const { isSuccess, firstRunWizardAvailable, usingDefaultPassword, refresh } = useSession()
  const settings = useSettings()
  const importCount = useImportLegacyCount()
  const [step, setStep] = useState<FirstRunWizardStep>("welcome")
  const [drafts, setDrafts] = useState<Drafts>({
    password: "",
    passwordConfirmation: "",
    llmKey: "",
    language: null,
    timezone: null,
  })

  /**
   * The exact values already written to the server. "Outstanding" is then a plain comparison
   * against this rather than a hand-maintained dirty flag — so editing an already-submitted field
   * makes it outstanding again on its own, which is what stops the last page's catch-all submit
   * from skipping an edit made after that field was first sent.
   */
  const [committed, setCommitted] = useState({ password: "", llmKey: "" })

  const updateDraft = (patch: Partial<Drafts>) => setDrafts((current) => ({ ...current, ...patch }))

  const passwordOutstanding =
    drafts.password !== "" && drafts.password !== committed.password
  const llmKeyOutstanding =
    drafts.llmKey.trim() !== "" && drafts.llmKey.trim() !== committed.llmKey
  const languageOutstanding =
    drafts.language !== null && drafts.language !== (settings.data?.language ?? "auto")
  const timezoneOutstanding =
    drafts.timezone !== null && drafts.timezone.trim() !== (settings.data?.timezone ?? "")

  useDocumentTitle(t("firstRunWizard.title") ?? undefined)

  /**
   * Which items are already handled, read straight off the same queries each step saves through —
   * so a successful save ticks its own tab without any step having to report upward, and an admin
   * who arrives with a key already configured sees that item ticked immediately.
   *
   * `welcome`/`done` are never ticked: one is an introduction and the other is the end of the flow,
   * and neither has anything that can be "handled".
   */
  const completed: Partial<Record<FirstRunWizardStep, boolean>> = {
    // Only the password half of this page can be "not handled": language and timezone always hold
    // a value (the server default), so there is nothing for a tick to say about them.
    basics: !usingDefaultPassword,
    llmKey: settings.data?.llm_api_key_set === true,
    import: (importCount.data?.import_count ?? 0) > 0,
  }

  // The explicit re-run was launched from Settings, so it should hand the user back there rather
  // than dumping them in the library — the wizard is a detour from that page, not a destination.
  const fromSettings = searchParams.get("from") === "settings"
  const exitTo = fromSettings ? routes.settings() : routes.library()

  const stepIndex = FIRST_RUN_WIZARD_STEPS.indexOf(step)
  /** Clamped rather than nullable: every step that renders a Back button sits past index 0, and a
   *  no-op at index 0 is simpler than threading an optional through four call sites. */
  const goBack = () => setStep(FIRST_RUN_WIZARD_STEPS[Math.max(0, stepIndex - 1)])

  /** Writes the password draft. Resolves to an error message, or `null` on success / nothing to do. */
  async function submitPassword(): Promise<string | null> {
    const problem = passwordStepProblem(drafts.password, drafts.passwordConfirmation)
    // Nothing typed: a no-op, not an error. Submitting an empty string would *clear* the password
    // (`settings::change_password` always writes whatever it is given, with no "blank keeps the
    // current one" convention), which is the opposite of this step's purpose.
    if (problem === "empty") return null
    if (problem === "mismatch") return t("settings.passwordsDonTMatch") ?? null
    try {
      await changePassword.mutateAsync(drafts.password)
      setCommitted((current) => ({ ...current, password: drafts.password }))
      // The status query drives both the library's default-password warning toast and the step's
      // own banner, so it has to be re-asked rather than left stale for up to
      // `AUTHENTICATED_RECHECK_INTERVAL_MS`. A refetch keeps the previous data, so `RequireAuth`
      // never flashes.
      void refresh()
      toast({ heading: t("firstRunWizard.passwordSaved") ?? undefined, icon: "success" })
      return null
    } catch (e) {
      return String(e)
    }
  }

  /** Writes the DeepSeek key draft. Resolves to an error message, or `null` on success / nothing. */
  async function submitLlmKey(): Promise<string | null> {
    const key = drafts.llmKey.trim()
    if (!key || key === committed.llmKey) return null
    try {
      await updateSettings.mutateAsync({ llm_api_key: key })
      setCommitted((current) => ({ ...current, llmKey: key }))
      toast({ heading: t("firstRunWizard.llmKeySaved") ?? undefined, icon: "success" })
      return null
    } catch (e) {
      return String(e)
    }
  }

  /** The basics page's "Next": both of the drafts that live on it, in one click, stopping at the
   *  first failure so the second is not written on top of a page the user has to come back to fix. */
  async function submitBasics(): Promise<string | null> {
    const passwordError = await submitPassword()
    if (passwordError) return passwordError
    const languageError = await submitLanguage()
    if (languageError) return languageError
    return submitTimezone()
  }

  /** Writes the language-order draft. Resolves to an error message, or `null` on success / nothing. */
  async function submitLanguage(): Promise<string | null> {
    if (drafts.language === null || !languageOutstanding) return null
    try {
      await updateSettings.mutateAsync({ language: drafts.language })
      return null
    } catch (e) {
      return String(e)
    }
  }

  /** Writes the timezone draft. Resolves to an error message, or `null` on success / nothing. */
  async function submitTimezone(): Promise<string | null> {
    const timezone = drafts.timezone?.trim()
    if (!timezone || !timezoneOutstanding) return null
    try {
      await updateSettings.mutateAsync({ timezone })
      return null
    } catch (e) {
      return String(e)
    }
  }

  /**
   * The last page's catch-all: writes whatever is still outstanding, and nothing else.
   *
   * A step that was already sent from its own Next is skipped rather than re-sent — repeatedly
   * re-writing the password would rotate nothing useful, and re-sending the key would re-trigger
   * the first-time-key backfill job for no reason.
   */
  async function submitOutstanding(): Promise<{ steps: string[]; error: string | null }> {
    const written: string[] = []
    // Captured before either write, because each one updates `committed` as it succeeds.
    if (passwordOutstanding) {
      const error = await submitPassword()
      if (error) return { steps: written, error: namedFailure("basics", error) }
      written.push(t("firstRunWizard.step.basics"))
    }
    if (llmKeyOutstanding) {
      const error = await submitLlmKey()
      if (error) return { steps: written, error: namedFailure("llmKey", error) }
      written.push(t("firstRunWizard.step.llmKey"))
    }
    if (languageOutstanding) {
      const error = await submitLanguage()
      if (error) return { steps: written, error: namedFailure("basics", error) }
      written.push(t("settings.language"))
    }
    if (timezoneOutstanding) {
      const error = await submitTimezone()
      if (error) return { steps: written, error: namedFailure("basics", error) }
      written.push(t("settings.timezone"))
    }
    return { steps: written, error: null }
  }

  /** Names the failing step in a submit error. Without it the last page reports a bare "Passwords
   *  don't match!" while showing no password field at all — the message has to say which tab to go
   *  back to, since the drafts it is complaining about were typed on a different page. */
  function namedFailure(step: "basics" | "llmKey", error: string): string {
    return (
      t("firstRunWizard.submitFailedOn", {
        step: t(`firstRunWizard.step.${step}`),
        error,
      }) ?? error
    )
  }

  /**
   * Records that this instance has had the wizard presented, then leaves. Awaited before
   * navigating so the session status the gate reads is refreshed first — but a failure here is
   * deliberately swallowed: being unable to write the marker (a Redis hiccup) must not strand the
   * user inside the wizard, and the worst consequence is one more auto-open next visit.
   */
  async function finish() {
    try {
      await updateSettings.mutateAsync({ first_run_wizard_done: true })
      await queryClient.invalidateQueries({ queryKey: SESSION_QUERY_KEY })
    } catch {
      // See above — leaving is the priority.
    }
    navigate(exitTo, { replace: true })
  }

  // Both early returns sit after every hook above, so the hook order is stable. Blank (not a
  // redirect) while the status is unresolved, matching `RequireAuth` — guessing here would bounce
  // a legitimate admin out on a single slow `/login/status`.
  if (!isSuccess) return null
  // A deployment that switched the wizard off (`--disable-first-run-wizard`) hides every entry
  // point into it, so reaching this route means a stale bookmark or hand-typed URL. Render nothing
  // of it rather than a wizard whose "finish" writes a marker nothing will ever read.
  if (!firstRunWizardAvailable) return <Navigate to={routes.library()} replace />

  return (
    <div className="ido">
      <h2 className="ih" style={{ textAlign: "center" }}>
        {t("firstRunWizard.title")}
      </h2>

      <WizardStepNav current={step} completed={completed} onGoTo={setStep} />

      {/* `textAlign: left` because legacy centres the whole document (`body { text-align: center }`,
          which is what its own table-based forms expect). A form reads wrong centred: labels float
          above the middle of their boxes. The page title and the step tabs keep their own centring;
          the one deliberately centred control is the first step's primary button. */}
      <div style={{ maxWidth: 720, margin: "0 auto", textAlign: "left" }}>
        {step === "welcome" && <WelcomeStep onContinue={() => setStep("basics")} />}
        {step === "basics" && (
          <BasicsStep
            password={drafts.password}
            confirmation={drafts.passwordConfirmation}
            onPasswordChange={(value) => updateDraft({ password: value })}
            onConfirmationChange={(value) => updateDraft({ passwordConfirmation: value })}
            language={drafts.language}
            onLanguageChange={(value) => updateDraft({ language: value })}
            timezone={drafts.timezone}
            onTimezoneChange={(value) => updateDraft({ timezone: value })}
            onSubmit={submitBasics}
            onBack={goBack}
            onContinue={() => setStep("llmKey")}
          />
        )}
        {step === "llmKey" && (
          <LlmKeyStep
            value={drafts.llmKey}
            onChange={(value) => updateDraft({ llmKey: value })}
            onSubmit={submitLlmKey}
            onBack={goBack}
            onContinue={() => setStep("import")}
          />
        )}
        {step === "import" && (
          <LegacyImportStep onBack={goBack} onContinue={() => setStep("done")} />
        )}
        {step === "done" && (
          <DoneStep
            fromSettings={fromSettings}
            onSubmitAll={submitOutstanding}
            onFinish={() => void finish()}
            onBack={goBack}
          />
        )}
      </div>

      {step !== "done" && (
        /* A plain `stdbtn`, like every other button in the app. It used to be a `ghost-btn`, which
           carries no font-size of its own and so rendered at the browser's 13.3px default next to
           12px buttons — it read as unstyled text rather than as something clickable. Centred via
           flex rather than `text-align`, since `.stdbtn` does not lay out as an inline box. */
        <div style={{ display: "flex", justifyContent: "center", marginTop: 24 }}>
          <Button variant="stdbtn" onClick={() => void finish()}>
            {t("firstRunWizard.skipWizard")}
          </Button>
        </div>
      )}
    </div>
  )
}

function WelcomeStep({ onContinue }: { onContinue: () => void }) {
  const { t } = useTranslation()
  return (
    <div>
      <h3>{t("firstRunWizard.welcomeHeading")}</h3>
      <p style={{ fontSize: FONT_SIZE_SM }}>{t("firstRunWizard.welcomeBody")}</p>
      <ul style={{ fontSize: FONT_SIZE_SM }}>
        <li>{t("firstRunWizard.welcomeItemPassword")}</li>
        <li>{t("firstRunWizard.welcomeItemLlmKey")}</li>
        <li>{t("firstRunWizard.welcomeItemImport")}</li>
      </ul>
      <p style={{ fontSize: FONT_SIZE_SM }}>{t("firstRunWizard.welcomeAllOptional")}</p>
      <StepActions primaryLabel={t("pluginWizard.nextStep")} onPrimary={onContinue} align="center" />
    </div>
  )
}

function DoneStep({
  fromSettings,
  onSubmitAll,
  onFinish,
  onBack,
}: {
  fromSettings: boolean
  onSubmitAll: () => Promise<{ steps: string[]; error: string | null }>
  onFinish: () => void
  onBack: () => void
}) {
  const { t, i18n } = useTranslation()
  const [error, setError] = useState("")
  const [busy, setBusy] = useState(false)

  /** `、` between CJK labels, `, ` elsewhere — this is the one place two step names are listed. */
  const listSeparator = /^(zh|ja|ko)/.test(i18n.language) ? "、" : ", "

  /**
   * Submit-then-leave, rather than a submit button next to a separate leave button: the whole point
   * of the catch-all is that nothing typed along the way gets dropped, and a "leave" that skips it
   * is exactly how something would still be forgotten.
   */
  async function handleSubmitAndFinish() {
    setError("")
    setBusy(true)
    try {
      const { steps, error: failure } = await onSubmitAll()
      if (failure) {
        setError(failure)
        return
      }
      // Reported as a toast rather than a line on the page: `onFinish` navigates away immediately,
      // so anything rendered here would never be read.
      if (steps.length === 0) {
        toast({ heading: t("firstRunWizard.nothingToSubmit") ?? undefined, icon: "info" })
      } else {
        toast({
          heading:
            t("firstRunWizard.submittedAll", { steps: steps.join(listSeparator) }) ?? undefined,
          icon: "success",
        })
      }
      onFinish()
    } finally {
      setBusy(false)
    }
  }

  return (
    <div>
      <h3>{t("firstRunWizard.doneHeading")}</h3>
      <p style={{ fontSize: FONT_SIZE_SM }}>{t("firstRunWizard.doneBody")}</p>
      <p style={{ fontSize: FONT_SIZE_SM }}>{t("firstRunWizard.submitAllHint")}</p>

      {error && (
        <p style={{ fontSize: FONT_SIZE_SM }} role="alert">
          {error}
        </p>
      )}

      <StepActions
        onBack={onBack}
        primaryLabel={
          fromSettings
            ? t("firstRunWizard.submitAndBackToSettings")
            : t("firstRunWizard.submitAndBackToLibrary")
        }
        onPrimary={() => void handleSubmitAndFinish()}
        busy={busy}
      />
    </div>
  )
}
