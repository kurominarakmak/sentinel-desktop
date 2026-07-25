import { Component } from "react";
import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppErrorBoundary } from "./AppErrorBoundary";

const unsafe = "/Users/example/private/project: SQL failure: raw stderr secret";
function ThrowOnRender(): never { throw new Error(unsafe); }
class ThrowOnMount extends Component { componentDidMount(): void { throw new Error(unsafe); } render() { return <p>lifecycle child</p>; } }

describe("AppErrorBoundary", () => {
  afterEach(() => vi.restoreAllMocks());
  it("renders a normal child", () => {
    render(<AppErrorBoundary><p>healthy child</p></AppErrorBoundary>);
    expect(screen.getByText("healthy child")).toBeInTheDocument(); expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
  it("shows only a safe fallback for a throwing render child", () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined); render(<AppErrorBoundary><ThrowOnRender /></AppErrorBoundary>);
    const fallback = screen.getByRole("alert"); expect(fallback).toHaveTextContent("Agent Sentinel could not start"); expect(screen.queryByText("healthy child")).not.toBeInTheDocument(); expect(fallback).not.toHaveTextContent(unsafe); expect(fallback).not.toHaveTextContent(/Users\/example|SQL|stderr/);
  });
  it("shows the same safe fallback for a throwing lifecycle child", () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined); render(<AppErrorBoundary><ThrowOnMount /></AppErrorBoundary>);
    const fallback = screen.getByRole("alert"); expect(fallback).toHaveTextContent("The prompt UI failed to render."); expect(fallback).not.toHaveTextContent(unsafe); expect(fallback).not.toHaveTextContent(/Users\/example|SQL|stderr/);
  });
});
