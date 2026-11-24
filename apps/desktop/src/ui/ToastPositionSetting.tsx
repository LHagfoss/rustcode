import { toast } from "../state/store";
import {
  DEFAULT_TOAST_POSITION,
  TOAST_POSITIONS,
  type ToastPosition,
  useToastPosition,
} from "../state/toastPosition";
import { Choice, Row } from "./settingsKit";

export function ToastPositionSetting() {
  const [position, setPosition] = useToastPosition();
  const pick = (next: ToastPosition) => {
    setPosition(next);
    toast("Toasts show here", "info", { id: "toast-position-sample" });
  };
  return (
    <Row
      title="Toast position"
      reset={position !== DEFAULT_TOAST_POSITION ? () => pick(DEFAULT_TOAST_POSITION) : undefined}
      subtitle="Where short notices like Copied or Pull request opened appear."
    >
      <Choice
        label="Toast position"
        value={position}
        onChange={pick}
        options={TOAST_POSITIONS.map((p) => ({ value: p.id, label: p.label }))}
      />
    </Row>
  );
}
