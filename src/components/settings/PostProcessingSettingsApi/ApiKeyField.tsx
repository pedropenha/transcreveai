import React, { useState } from "react";
import { Input } from "../../ui/Input";
import { apiKeyEditValue } from "./apiKeyEdit";

interface ApiKeyFieldProps {
  /**
   * Display value — a masked vault hint ("••••1234") when a key is saved, or
   * empty. It is never the real key: the frontend has no way to read one.
   */
  value: string;
  /**
   * Called with the raw typed text on blur. An unchanged hint produces the
   * hint itself (the caller treats it as a no-op); an empty string clears the
   * key; anything else replaces it via `secret_set`.
   */
  onBlur: (value: string) => void;
  disabled: boolean;
  placeholder?: string;
  className?: string;
}

export const ApiKeyField: React.FC<ApiKeyFieldProps> = React.memo(
  ({ value, onBlur, disabled, placeholder, className = "" }) => {
    const [localValue, setLocalValue] = useState(value);

    // Sync with prop changes
    React.useEffect(() => {
      setLocalValue(value);
    }, [value]);

    const handleChange = (event: React.ChangeEvent<HTMLInputElement>) => {
      // Bullet characters can only come from the masked hint itself — a value
      // that still contains them is a mangled mask (e.g. a mid-mask
      // backspace), never new key material, so the edit is discarded and the
      // hint restored instead of writing "123" into the vault.
      setLocalValue(apiKeyEditValue(value, event.target.value));
    };

    // Select the whole mask on focus so typing replaces it outright instead of
    // appending inside it.
    const handleFocus = (event: React.FocusEvent<HTMLInputElement>) => {
      if (localValue === value && value) {
        event.target.select();
      }
    };

    return (
      <Input
        type="password"
        value={localValue}
        onChange={handleChange}
        onFocus={handleFocus}
        onBlur={() => onBlur(localValue)}
        placeholder={placeholder}
        variant="compact"
        disabled={disabled}
        className={`flex-1 min-w-[320px] ${className}`}
      />
    );
  },
);

ApiKeyField.displayName = "ApiKeyField";
