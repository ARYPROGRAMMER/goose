import { Message } from '../types/message';
import { getApiUrl } from '../config';
import { FullExtensionConfig } from '../extensions';

export interface Parameter {
  key: string;
  description: string;
  input_type: 'string' | 'select' | 'number' | 'boolean';
  default?: string;
  requirement: 'required' | 'optional' | 'user_prompt';
  options?: string[]; // For select input type
}

export interface Recipe {
  title: string;
  description: string;
  instructions: string;
  prompt?: string;
  activities?: string[];
  parameters?: Parameter[];
  author?: {
    contact?: string;
    metadata?: string;
  };
  extensions?: FullExtensionConfig[];
  goosehints?: string;
  context?: string[];
  profile?: string;
  mcps?: number;
  version?: string;
  // Sub recipes support
  sub_recipes?: Array<{
    name: string;
    path: string;
    values?: Record<string, unknown>;
    description?: string;
  }>;
  // Response schema - typically a JSON schema structure
  response?:
    | {
        json_schema?: {
          type?: string;
          properties?: Record<
            string,
            {
              type?: string;
              description?: string;
              items?: {
                type?: string;
              };
            }
          >;
          required?: string[];
        };
      }
    | string; // Can also be a string representation
  // Properties added for scheduled execution
  scheduledJobId?: string;
  isScheduledExecution?: boolean;
}

export interface CreateRecipeRequest {
  messages: Message[];
  title: string;
  description: string;
  activities?: string[];
  author?: {
    contact?: string;
    metadata?: string;
  };
}

export interface CreateRecipeResponse {
  recipe: Recipe | null;
  error: string | null;
}

export async function createRecipe(request: CreateRecipeRequest): Promise<CreateRecipeResponse> {
  const url = getApiUrl('/recipe/create');
  console.log('Creating recipe at:', url);
  console.log('Request:', JSON.stringify(request, null, 2));

  const response = await fetch(url, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(request),
  });

  if (!response.ok) {
    const errorText = await response.text();
    console.error('Failed to create recipe:', {
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(`Failed to create recipe: ${response.statusText} (${errorText})`);
  }

  return response.json();
}
