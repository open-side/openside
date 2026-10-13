import { Button, Dialog, DialogActions } from "../ui";

/**
 * Confirms stopping running work. `all` is the composer Stop: it ends the agent and every Task
 * it started. `task` cancels one Task and leaves the agent and the other Tasks running. A
 * stopped run cannot be resumed where it left off.
 */
export type StopScope = { kind: "all"; tasks: number } | { kind: "task"; name?: string };

function copy(scope: StopScope): { title: string; description: string; confirm: string } {
  if (scope.kind === "task") {
    return {
      title: "Stop this task?",
      description: `${scope.name ? `“${scope.name}” stops` : "This task stops"} where it is. The agent and any other tasks keep running, and anything already done stays done.`,
      confirm: "Stop task",
    };
  }
  return {
    title: "Stop everything?",
    description: `${scope.tasks > 0 ? `Stops the agent and its ${scope.tasks === 1 ? "running task" : `${scope.tasks} running tasks`}` : "Stops the agent"} where ${scope.tasks > 0 ? "they are" : "it is"}. Anything already done stays done.`,
    confirm: "Stop everything",
  };
}

export function StopTaskDialog({ open, scope, onClose, onConfirm }: { open: boolean; scope: StopScope; onClose: () => void; onConfirm: () => void }) {
  const { title, description, confirm } = copy(scope);
  return (
    <Dialog open={open} onClose={onClose} title={title} description={description} className="stop-task-dialog">
      <DialogActions>
        <Button onClick={onClose}>Keep running</Button>
        <Button
          variant="danger"
          onClick={() => {
            onClose();
            onConfirm();
          }}
        >
          {confirm}
        </Button>
      </DialogActions>
    </Dialog>
  );
}
