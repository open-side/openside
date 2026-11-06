import { t } from "../i18n";
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
      title: t("thread.stop.task.title"),
      description: scope.name ? t("thread.stop.task.descriptionNamed", { name: scope.name }) : t("thread.stop.task.description"),
      confirm: t("thread.stop.task.confirm"),
    };
  }
  return {
    title: t("thread.stop.all.title"),
    description: scope.tasks > 0 ? t("thread.stop.all.descriptionWithTasks", { count: scope.tasks }) : t("thread.stop.all.description"),
    confirm: t("thread.stop.all.confirm"),
  };
}

export function StopTaskDialog({ open, scope, onClose, onConfirm }: { open: boolean; scope: StopScope; onClose: () => void; onConfirm: () => void }) {
  const { title, description, confirm } = copy(scope);
  return (
    <Dialog open={open} onClose={onClose} title={title} description={description} className="stop-task-dialog">
      <DialogActions>
        <Button onClick={onClose}>{t("thread.stop.keepRunning")}</Button>
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
