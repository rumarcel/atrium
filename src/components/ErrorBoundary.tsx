import { Component, type ErrorInfo, type ReactNode } from "react";

export interface ErrorBoundaryCopy {
  eyebrow: string;
  title: string;
  description: string;
  reload: string;
}

interface ErrorBoundaryProps {
  children: ReactNode;
  copy?: ErrorBoundaryCopy;
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
      const copy = this.props.copy ?? {
        eyebrow: "Personal Hub",
        title: "Something went wrong.",
        description:
          "The dashboard could not be rendered. Restart the app to try again.",
        reload: "Reload dashboard",
      };

      return (
        <main className="error-screen">
          <div className="error-screen__mark" aria-hidden="true">
            PH
          </div>
          <p className="eyebrow">{copy.eyebrow}</p>
          <h1>{copy.title}</h1>
          <p>{copy.description}</p>
          <button type="button" onClick={() => window.location.reload()}>
            {copy.reload}
          </button>
        </main>
      );
    }

    return this.props.children;
  }
}
