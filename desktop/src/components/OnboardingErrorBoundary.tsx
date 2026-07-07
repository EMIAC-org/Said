import React from "react";
import { clearOnboardingProgress } from "@/lib/onboardingProgress";
import { CopyableError, describeError } from "@/components/CopyableError";

interface Props {
  children: React.ReactNode;
}

interface State {
  error: Error | null;
  componentStack: string;
}

/**
 * Catches any render/runtime error thrown inside the onboarding flow so a single
 * bad state never white-screens the whole app (the app has no top-level
 * boundary). Offers a "Restart setup" recovery that clears the persisted
 * onboarding progress and reloads, plus a plain reload that keeps progress.
 */
export class OnboardingErrorBoundary extends React.Component<Props, State> {
  constructor(props: Props) {
    super(props);
    this.state = { error: null, componentStack: "" };
  }

  static getDerivedStateFromError(error: Error): State {
    return { error, componentStack: "" };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    // Surface to the console/log; onboarding runs before telemetry consent so we
    // deliberately do not report this anywhere off-device.
    console.error("[onboarding] uncaught error", error, info.componentStack);
    this.setState({ componentStack: info.componentStack ?? "" });
  }

  private restart = () => {
    clearOnboardingProgress();
    window.location.reload();
  };

  private reload = () => {
    window.location.reload();
  };

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="onb-error-screen">
        <div className="onb-error-card">
          <h2 className="onb-error-title">Setup hit a snag</h2>
          <p className="onb-error-desc">
            Something went wrong while setting up AirNote. Your progress is saved — you can
            reload and pick up where you left off, or restart setup from the beginning.
          </p>
          <div style={{ margin: "12px 0" }}>
            <CopyableError
              title="Exact error (copy and send this):"
              detail={[describeError(this.state.error), this.state.componentStack]
                .filter(Boolean)
                .join("\n\nComponent stack:\n")}
            />
          </div>
          <div className="onb-error-actions">
            <button onClick={this.reload} className="btn-primary btn-lg w-full">
              Reload and continue
            </button>
            <button type="button" onClick={this.restart} className="onb-skip-link">
              Restart setup from the beginning
            </button>
          </div>
        </div>
      </div>
    );
  }
}
