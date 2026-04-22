import daisyui from "daisyui";
import containerQueries from "@tailwindcss/container-queries";
import daisyuiThemes from "daisyui/src/theming/themes";

/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  darkMode: 'class',
  theme: {
    extend: {},
  },
  plugins: [daisyui, containerQueries],
  daisyui: {
    themes: [
      "light",
      {
        dark: {
          ...daisyuiThemes["dark"],
          "base-content": "#ECF9FF",
        },
      },
    ],
    darkTheme: "dark",
  },
};
