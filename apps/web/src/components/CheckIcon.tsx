import { Check } from "lucide-react";

type Props = {
  size?: number;
  strokeWidth?: number;
};

export default function CheckIcon({ size = 16, strokeWidth = 2.5 }: Props) {
  return <Check size={size} strokeWidth={strokeWidth} aria-hidden="true" focusable="false" />;
}
