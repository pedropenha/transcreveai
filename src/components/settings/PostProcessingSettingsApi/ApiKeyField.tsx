import React, { useState } from "react";
import { Input } from "../../ui/Input";

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
      let next = event.target.value;
      if (localValue === value && value) {
        // The user is editing on top of the mask. Strip a leading untouched
        // copy of the hint so only newly typed text is submitted, and drop any
        // residual mask characters from mid-string edits — bullets must never
        // be written to the vault as if they were key material.
        if (next.startsWith(value)) {
          next = next.slice(value.length);
        }
        next = next.replace(/•/g, "");
      }
      setLocalValue(next);
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
