import { Link } from "react-router";
import { useClient } from "../context";
import { latestTaskState, useTaskThumbnail } from "../hooks/useTaskThumbnail";
import { useSessionView } from "../hooks/useSessionView";
import type { SessionView, SubagentView } from "../state/session";
import { taskLine, taskStateLabel } from "./taskStatus";
import { browserLine } from "./toolStatus";

const MAX_TASKS = 6;

/**
 * The assistant's background Tasks, newest first, as cards that lead into its thread. Built from
 * the thread's event stream; the API keeps no Task timestamps, so cards carry no times. The whole
 * section is absent until there is a Task.
 */
export function RecentTasks({ sessionId, to }: { sessionId: string; to: string }) {
  const { view } = useSessionView(useClient(), sessionId);
  const tasks = view.subagents
    .filter((task) => task.background)
    .slice(-MAX_TASKS)
    .reverse();
  if (tasks.length === 0) return null;
  return (
    <section className="recent-tasks" aria-labelledby="recent-tasks-heading">
      <h2 id="recent-tasks-heading" className="section-heading">
        Recent tasks
      </h2>
      <ul className="recent-tasks-list">
        {tasks.map((task) => (
          <li key={task.id}>
            <TaskCard sessionId={sessionId} view={view} task={task} to={to} />
          </li>
        ))}
      </ul>
    </section>
  );
}

function TaskCard({ sessionId, view, task, to }: { sessionId: string; view: SessionView; task: SubagentView; to: string }) {
  const state = latestTaskState(view, task.id);
  const thumbnail = useTaskThumbnail(sessionId, state?.thumbnail ?? undefined, true);
  const status = taskStateLabel(task);
  const line = task.status === "running" && state ? browserLine(state) : taskLine(task);
  return (
    <Link to={to} className="task-card" data-status={task.status}>
      <span className="task-card-status">{status}</span>
      <span className="task-card-title">{task.task}</span>
      {line && line !== status ? <span className="task-card-line">{line}</span> : null}
      {thumbnail ? <img className="task-card-thumb" src={thumbnail} alt="" /> : null}
    </Link>
  );
}
