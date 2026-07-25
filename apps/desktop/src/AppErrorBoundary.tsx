import { Component, type ErrorInfo, type ReactNode } from "react";
type State = { failed: boolean };
export class AppErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { failed: false };
  static getDerivedStateFromError(): State { return { failed: true }; }
  componentDidCatch(_error: Error, _info: ErrorInfo) { /* keep internal details out of the prompt */ }
  render() { return this.state.failed ? <main className="fatal-error" role="alert"><h1>Agent Sentinel could not start</h1><p>The prompt UI failed to render. Please reopen the prompt.</p></main> : this.props.children; }
}
