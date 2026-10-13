import { ApiError } from "../api/client";
import { errorMessage } from "../hooks/useResource";
import { LocalOnlyNotice } from "../remote/LocalOnlyNotice";
import { ErrorNote } from "./blocks";

/** A failed save; `localOnly` when only the computer's own authority may make the change. */
export interface Failure {
  message: string;
  localOnly: boolean;
}

export function toFailure(cause: unknown): Failure {
  return { message: errorMessage(cause), localOnly: cause instanceof ApiError && cause.code === "local_access_required" };
}

/** The error under a form: a plain note, or the explanation of where to make the change. */
export function FailureNote({ failure, what, onRetry }: { failure: Failure | null; what: string; onRetry: () => void }) {
  if (!failure) return null;
  return failure.localOnly ? <LocalOnlyNotice what={what} onChanged={onRetry} /> : <ErrorNote>{failure.message}</ErrorNote>;
}
