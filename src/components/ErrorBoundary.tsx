import { Component, type ErrorInfo, type ReactNode } from "react";

interface ErrorBoundaryProps {
  children: ReactNode;
}

interface ErrorBoundaryState {
  hasError: boolean;
}

export class ErrorBoundary extends Component<
  ErrorBoundaryProps,
  ErrorBoundaryState
> {
  state: ErrorBoundaryState = { hasError: false };

  static getDerivedStateFromError(): ErrorBoundaryState {
    return { hasError: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Personal Hub failed to render", error, info);
  }

  render() {
    if (this.state.hasError) {
      return (
        <main className="error-screen">
          <div className="error-screen__mark" aria-hidden="true">
            PH
          </div>
          <p className="eyebrow">Personal Hub</p>
          <h1>Something went wrong.</h1>
          <p>The dashboard could not be rendered. Restart the app to try again.</p>
          <button type="button" onClick={() => window.location.reload()}>
            Reload dashboard
          </button>
        </main>
      );
    }

    return this.props.children;
  }
}
